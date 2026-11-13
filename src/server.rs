//! Bounded RESP2 connections and connection-local commands.

use std::{
    future::Future,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
    task::{JoinError, JoinSet},
    time::{MissedTickBehavior, timeout, timeout_at},
};

use crate::{
    config::{Config, MAX_PASSWORD_BYTES},
    db::Db,
    protocol::{self, Frame},
};

const PIPELINE_QUANTUM: usize = 32;
const MAX_METADATA_BYTES: usize = 256;
const MAX_CLIENTS_ERROR: &[u8] = b"-ERR max number of clients reached\r\n";
static NEXT_CLIENT_ID: AtomicI64 = AtomicI64::new(1);

/// Serve an already-bound listener until shutdown, then stop accepting, cancel
/// connections and join every task. No lock is held across network I/O.
///
/// Cancelling this future abruptly aborts its owned `JoinSet`; callers wanting
/// completed teardown should signal `shutdown` and await this function instead.
pub async fn serve(
    listener: TcpListener,
    config: Config,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    config.validate()?;
    let config = Arc::new(config);
    let db = Arc::new(Mutex::new(Db::new(config.max_memory)));
    let admission = Arc::new(Semaphore::new(config.max_clients));
    let (stop, stopped) = watch::channel(false);
    let mut tasks = JoinSet::new();
    let mut cleanup = tokio::time::interval(Duration::from_millis(100));
    cleanup.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);

    let result = loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break Ok(()),
            Some(result) = tasks.join_next(), if !tasks.is_empty() => observe_task(result),
            _ = cleanup.tick() => {
                match db.lock() {
                    Ok(mut db) => db.expire(Instant::now(), 256),
                    Err(_) => break Err(io::Error::other("database lock poisoned")),
                }
            }
            accepted = listener.accept() => {
                let (mut stream, _) = match accepted {
                    Ok(connection) => connection,
                    Err(error) => break Err(error),
                };
                let permit = match admission.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        // Rejections never spawn tasks or wait behind admitted clients.
                        tokio::select! {
                            biased;
                            () = &mut shutdown => break Ok(()),
                            result = timeout(Duration::from_millis(100), stream.write_all(MAX_CLIENTS_ERROR)) => {
                                if !matches!(result, Ok(Ok(()))) {
                                    tracing::debug!(category = "admission_write", "client rejected");
                                }
                            }
                        }
                        continue;
                    }
                };
                let id = match NEXT_CLIENT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)) {
                    Ok(id) => id,
                    Err(_) => break Err(io::Error::other("client ID space exhausted")),
                };
                let config = config.clone();
                let db = db.clone();
                let mut stopped = stopped.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        biased;
                        _ = stopped.changed() => Ok(()),
                        result = connection(stream, &config, &db, id) => result,
                    }
                });
            }
        }
    };

    drop(listener);
    stop.send_replace(true);
    let drain = async {
        while let Some(result) = tasks.join_next().await {
            observe_task(result);
        }
    };
    if timeout(Duration::from_secs(config.shutdown_timeout_secs), drain)
        .await
        .is_err()
    {
        tasks.abort_all();
        // Abort requests are not completion; retain ownership until every join.
        while let Some(result) = tasks.join_next().await {
            observe_task(result);
        }
    }
    result
}

fn observe_task(result: Result<io::Result<()>, JoinError>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!(category = ?error.kind(), "connection closed"),
        Err(error) if error.is_cancelled() => {}
        Err(_) => tracing::error!(category = "connection_task", "connection task failed"),
    }
}

async fn connection(
    mut stream: TcpStream,
    config: &Config,
    db: &Mutex<Db>,
    id: i64,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let mut client = Client {
        id,
        authenticated: config.password.is_none(),
        name: None,
        lib_name: None,
        lib_ver: None,
    };
    let mut input = Vec::new();
    let mut output = Vec::new();
    let mut offset = 0;
    let mut quantum = 0;
    let mut deadline = None;
    let mut scratch = [0_u8; 8192];

    loop {
        match protocol::decode(&input[offset..], config.limits()) {
            Ok(Some((args, consumed))) => {
                offset += consumed;
                deadline = None;
                let (frame, close) = execute(&args, &mut client, config, db)?;
                send(&mut stream, &frame, &mut output, config).await?;
                if close {
                    return Ok(());
                }
                quantum += 1;
                if quantum == PIPELINE_QUANTUM {
                    quantum = 0;
                    tokio::task::yield_now().await;
                }
                continue;
            }
            Err(_) => {
                // Protocol diagnostics must not echo input (which may be AUTH).
                send(
                    &mut stream,
                    &error("ERR protocol error"),
                    &mut output,
                    config,
                )
                .await?;
                return Err(io::Error::new(io::ErrorKind::InvalidData, "protocol error"));
            }
            Ok(None) => {}
        }

        if offset != 0 {
            input.copy_within(offset.., 0);
            input.truncate(input.len() - offset);
            offset = 0;
        }
        let remaining = config.max_frame_bytes - input.len();
        if remaining == 0 {
            send(
                &mut stream,
                &error("ERR request frame too large"),
                &mut output,
                config,
            )
            .await?;
            return Err(io::Error::new(io::ErrorKind::InvalidData, "input limit"));
        }
        // The slice bounds the read before any growth of the input allocation.
        let read_limit = remaining.min(scratch.len());
        let expires =
            *deadline.get_or_insert_with(|| tokio::time::Instant::now() + config.io_timeout());
        let read = timeout_at(expires, stream.read(&mut scratch[..read_limit]))
            .await
            .map_err(|_| timed_out())??;
        if read == 0 {
            return Ok(());
        }
        input
            .try_reserve_exact(read)
            .map_err(|_| io::Error::other("input allocation failed"))?;
        input.extend_from_slice(&scratch[..read]);
    }
}

/// Preflight the encoded size before allocating; there is only one response in
/// flight, and a write timeout closes the socket rather than retrying a mutation.
async fn send(
    stream: &mut TcpStream,
    frame: &Frame,
    output: &mut Vec<u8>,
    config: &Config,
) -> io::Result<()> {
    let length = frame.encoded_len();
    if length > config.max_output_bytes {
        return Err(io::Error::other("output limit"));
    }
    output.clear();
    output
        .try_reserve_exact(length)
        .map_err(|_| io::Error::other("output allocation failed"))?;
    frame.encode(output);
    timeout(config.io_timeout(), stream.write_all(output))
        .await
        .map_err(|_| timed_out())?
}

struct Client {
    id: i64,
    authenticated: bool,
    name: Option<Bytes>,
    lib_name: Option<Bytes>,
    lib_ver: Option<Bytes>,
}

fn execute(
    args: &[Bytes],
    client: &mut Client,
    config: &Config,
    db: &Mutex<Db>,
) -> io::Result<(Frame, bool)> {
    let command = args[0].as_ref();
    let frame = if command.eq_ignore_ascii_case(b"AUTH") {
        authenticate(args, client, config)
    } else if command.eq_ignore_ascii_case(b"HELLO") {
        hello(args, client, config)
    } else if command.eq_ignore_ascii_case(b"QUIT") {
        return Ok(if args.len() == 1 {
            (ok(), true)
        } else {
            (arity(), false)
        });
    } else if !client.authenticated {
        error("NOAUTH Authentication required")
    } else if command.eq_ignore_ascii_case(b"PING") {
        match args.len() {
            1 => Frame::Simple("PONG".into()),
            2 => Frame::Bulk(args[1].clone()),
            _ => arity(),
        }
    } else if command.eq_ignore_ascii_case(b"ECHO") {
        if args.len() == 2 {
            Frame::Bulk(args[1].clone())
        } else {
            arity()
        }
    } else if command.eq_ignore_ascii_case(b"SELECT") {
        if args.len() != 2 {
            arity()
        } else {
            match std::str::from_utf8(&args[1])
                .ok()
                .and_then(|index| index.parse::<i64>().ok())
            {
                Some(0) => ok(),
                Some(_) => error("ERR DB index is out of range"),
                None => error("ERR value is not an integer or out of range"),
            }
        }
    } else if command.eq_ignore_ascii_case(b"CLIENT") {
        client_command(args, client)
    } else {
        let mut db = db
            .lock()
            .map_err(|_| io::Error::other("database lock poisoned"))?;
        db.execute(args, Instant::now())
    };
    Ok((frame, false))
}

fn authenticate(args: &[Bytes], client: &mut Client, config: &Config) -> Frame {
    let (user, candidate) = match args {
        [_, password] => (b"default".as_slice(), password.as_ref()),
        [_, user, password] => (user.as_ref(), password.as_ref()),
        _ => return arity(),
    };
    let Some(password) = &config.password else {
        return error("ERR AUTH called without any password configured");
    };
    client.authenticated = password_matches(password, candidate) & (user == b"default");
    if client.authenticated {
        ok()
    } else {
        wrong_password()
    }
}

/// Pad to a fixed public bound so subtle's slice-length short circuit does not
/// expose the configured password length. Candidate length is untrusted/public.
fn password_matches(expected: &[u8], candidate: &[u8]) -> bool {
    if candidate.len() > MAX_PASSWORD_BYTES {
        return false;
    }
    let mut left = [0_u8; MAX_PASSWORD_BYTES];
    let mut right = [0_u8; MAX_PASSWORD_BYTES];
    left[..expected.len()].copy_from_slice(expected);
    right[..candidate.len()].copy_from_slice(candidate);
    bool::from(left.ct_eq(&right) & expected.len().ct_eq(&candidate.len()))
}

/// Stage all HELLO options before authenticating or changing metadata. RESP3,
/// malformed options, failed AUTH and invalid names leave the client untouched.
fn hello(args: &[Bytes], client: &mut Client, config: &Config) -> Frame {
    if args.len() > 1 && args[1].as_ref() != b"2" {
        return error("NOPROTO unsupported protocol version");
    }
    let mut auth = None;
    let mut name = None;
    let mut index = 2;
    while index < args.len() {
        if args[index].eq_ignore_ascii_case(b"AUTH") && auth.is_none() && index + 2 < args.len() {
            auth = Some((&args[index + 1], &args[index + 2]));
            index += 3;
        } else if args[index].eq_ignore_ascii_case(b"SETNAME")
            && name.is_none()
            && index + 1 < args.len()
        {
            if !valid_metadata(&args[index + 1]) {
                return error("ERR invalid client name");
            }
            name = Some(args[index + 1].clone());
            index += 2;
        } else {
            return error("ERR syntax error");
        }
    }
    let authenticated = match auth {
        Some((user, candidate)) => {
            let Some(password) = &config.password else {
                return error("ERR AUTH called without any password configured");
            };
            if !(password_matches(password, candidate) & (user.as_ref() == b"default")) {
                return wrong_password();
            }
            true
        }
        None => client.authenticated,
    };
    if !authenticated {
        return error("NOAUTH Authentication required");
    }
    client.authenticated = authenticated;
    if let Some(name) = name {
        client.name = (!name.is_empty()).then_some(name);
    }
    Frame::Array(vec![
        bulk(b"server"),
        bulk(b"remik"),
        bulk(b"version"),
        bulk(env!("CARGO_PKG_VERSION").as_bytes()),
        bulk(b"proto"),
        Frame::Integer(2),
        bulk(b"id"),
        Frame::Integer(client.id),
        bulk(b"mode"),
        bulk(b"standalone"),
        bulk(b"role"),
        bulk(b"master"),
        bulk(b"modules"),
        Frame::Array(Vec::new()),
    ])
}

fn client_command(args: &[Bytes], client: &mut Client) -> Frame {
    let Some(subcommand) = args.get(1) else {
        return arity();
    };
    if subcommand.eq_ignore_ascii_case(b"ID") {
        if args.len() == 2 {
            Frame::Integer(client.id)
        } else {
            arity()
        }
    } else if subcommand.eq_ignore_ascii_case(b"GETNAME") {
        if args.len() == 2 {
            client.name.clone().map_or(Frame::Null, Frame::Bulk)
        } else {
            arity()
        }
    } else if subcommand.eq_ignore_ascii_case(b"SETNAME") {
        if args.len() != 3 {
            return arity();
        }
        if !valid_metadata(&args[2]) {
            return error("ERR invalid client name");
        }
        client.name = (!args[2].is_empty()).then(|| args[2].clone());
        ok()
    } else if subcommand.eq_ignore_ascii_case(b"SETINFO") {
        if args.len() != 4 {
            return arity();
        }
        if !valid_metadata(&args[3]) {
            return error("ERR invalid client metadata");
        }
        let slot = if args[2].eq_ignore_ascii_case(b"LIB-NAME") {
            &mut client.lib_name
        } else if args[2].eq_ignore_ascii_case(b"LIB-VER") {
            &mut client.lib_ver
        } else {
            return error("ERR unsupported CLIENT SETINFO attribute");
        };
        *slot = Some(args[3].clone());
        ok()
    } else {
        error("ERR unsupported CLIENT subcommand")
    }
}

fn valid_metadata(value: &[u8]) -> bool {
    value.len() <= MAX_METADATA_BYTES && value.iter().all(|byte| (b'!'..=b'~').contains(byte))
}

fn ok() -> Frame {
    Frame::Simple("OK".into())
}
fn error(message: &str) -> Frame {
    Frame::Error(message.into())
}
fn arity() -> Frame {
    error("ERR wrong number of arguments")
}
fn wrong_password() -> Frame {
    error("WRONGPASS invalid username-password pair")
}
fn bulk(value: &'static [u8]) -> Frame {
    Frame::Bulk(Bytes::from_static(value))
}
fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "I/O deadline exceeded")
}

/// Probe AUTH (when configured) and PING within one finite deadline. Wildcard
/// listen addresses are translated to loopback for Docker/container healthchecks.
pub async fn healthcheck(config: &Config) -> io::Result<()> {
    config.validate()?;
    let mut address = config.bind;
    if address.ip().is_unspecified() {
        address.set_ip(match address.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    timeout(config.io_timeout(), async {
        let mut stream = TcpStream::connect(address).await?;
        if let Some(password) = &config.password {
            let mut request = Vec::new();
            Frame::Array(vec![
                bulk(b"AUTH"),
                Frame::Bulk(Bytes::copy_from_slice(password)),
            ])
            .encode(&mut request);
            stream.write_all(&request).await?;
            let mut response = [0_u8; 5];
            stream.read_exact(&mut response).await?;
            if response != *b"+OK\r\n" {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "healthcheck authentication failed",
                ));
            }
        }
        stream.write_all(b"*1\r\n$4\r\nPING\r\n").await?;
        let mut response = [0_u8; 7];
        stream.read_exact(&mut response).await?;
        if response != *b"+PONG\r\n" {
            return Err(io::Error::other("healthcheck did not receive PONG"));
        }
        Ok(())
    })
    .await
    .map_err(|_| timed_out())?
}
