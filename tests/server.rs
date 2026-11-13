use std::{
    future::Future,
    io::{self, Read, Write},
    net::SocketAddr,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use remik::{
    config::Config,
    protocol::Frame,
    server::{healthcheck, serve},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Barrier, oneshot},
    task::{JoinHandle, JoinSet},
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(5);

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    timeout(WAIT, future)
        .await
        .expect("operation exceeded test deadline")
}

struct Server {
    config: Config,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}

impl Server {
    async fn start(mut config: Config) -> Self {
        let listener = bounded(TcpListener::bind("127.0.0.1:0")).await.unwrap();
        config.bind = listener.local_addr().unwrap();
        let (shutdown, receiver) = oneshot::channel();
        let settings = config.clone();
        let task = tokio::spawn(serve(listener, settings, async {
            receiver
                .await
                .expect("server test dropped its shutdown sender");
        }));
        // The listener is bound before spawning, so connects need no guessed sleep.
        Self {
            config,
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(self.config.bind).await
    }

    async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        bounded(&mut self.task).await.unwrap().unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // A failed assertion must not detach the server or its owned connections.
        self.task.abort();
    }
}

struct Client(BufReader<TcpStream>);

impl Client {
    async fn connect(address: SocketAddr) -> Self {
        let stream = bounded(TcpStream::connect(address)).await.unwrap();
        stream.set_nodelay(true).unwrap();
        Self(BufReader::new(stream))
    }

    async fn write(&mut self, bytes: &[u8]) {
        bounded(self.0.get_mut().write_all(bytes)).await.unwrap();
    }

    async fn command(&mut self, args: &[&[u8]]) -> Frame {
        self.write(&request(args)).await;
        self.frame().await
    }

    async fn line(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        assert_ne!(
            bounded(self.0.read_until(b'\n', &mut bytes)).await.unwrap(),
            0
        );
        assert!(bytes.len() <= 1024);
        assert!(bytes.ends_with(b"\r\n"));
        bytes.truncate(bytes.len() - 2);
        bytes
    }

    async fn frame(&mut self) -> Frame {
        let line = self.line().await;
        match line[0] {
            b'+' => Frame::Simple(String::from_utf8(line[1..].to_vec()).unwrap()),
            b'-' => Frame::Error(String::from_utf8(line[1..].to_vec()).unwrap()),
            b':' => Frame::Integer(std::str::from_utf8(&line[1..]).unwrap().parse().unwrap()),
            b'$' if line == b"$-1" => Frame::Null,
            b'$' => {
                let length: usize = std::str::from_utf8(&line[1..]).unwrap().parse().unwrap();
                assert!(length <= 2 * 1024 * 1024);
                let mut bytes = vec![0; length + 2];
                bounded(self.0.read_exact(&mut bytes)).await.unwrap();
                assert_eq!(&bytes[length..], b"\r\n");
                bytes.truncate(length);
                Frame::Bulk(Bytes::from(bytes))
            }
            b'*' => {
                let count: usize = std::str::from_utf8(&line[1..]).unwrap().parse().unwrap();
                assert!(count <= 128);
                let mut values = Vec::new();
                for _ in 0..count {
                    values.push(Box::pin(self.frame()).await);
                }
                Frame::Array(values)
            }
            marker => panic!("unexpected RESP marker {marker}"),
        }
    }

    async fn closed(&mut self) {
        let mut byte = [0];
        match bounded(self.0.read(&mut byte)).await {
            Ok(0) => {}
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
            other => panic!("expected closed socket, got {other:?}"),
        }
    }
}

// Build requests independently of the production encoder.
fn request(args: &[&[u8]]) -> Vec<u8> {
    let mut bytes = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        bytes.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        bytes.extend_from_slice(arg);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes
}

fn ok() -> Frame {
    Frame::Simple("OK".into())
}
fn pong() -> Frame {
    Frame::Simple("PONG".into())
}
fn bulk(value: &[u8]) -> Frame {
    Frame::Bulk(Bytes::copy_from_slice(value))
}

fn error_starts(frame: Frame, prefix: &str) {
    assert!(
        matches!(&frame, Frame::Error(message) if message.starts_with(prefix)),
        "{frame:?}"
    );
}

#[tokio::test]
async fn fragments_binary_pipeline_and_quit_preserve_order() {
    let server = Server::start(Config::default()).await;
    let mut client = server.client().await;
    let value = b"\0\r\n\xffbinary";
    let set = request(&[b"SET", b"\0key", value]);
    for part in set.chunks(3) {
        client.write(part).await;
    }
    assert_eq!(client.frame().await, ok());
    let pipeline = [
        request(&[b"GET", b"\0key"]),
        request(&[b"ECHO", value]),
        request(&[b"PING", b""]),
        request(&[b"PING"]),
        request(&[b"QUIT"]),
        request(&[b"SET", b"after-quit", b"bad"]),
    ]
    .concat();
    client.write(&pipeline).await;
    assert_eq!(client.frame().await, bulk(value));
    assert_eq!(client.frame().await, bulk(value));
    assert_eq!(client.frame().await, bulk(b""));
    assert_eq!(client.frame().await, pong());
    assert_eq!(client.frame().await, ok());
    client.closed().await;
    let mut observer = server.client().await;
    assert_eq!(
        observer.command(&[b"GET", b"after-quit"]).await,
        Frame::Null
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_increments_are_atomic() {
    let server = Server::start(Config::default()).await;
    let barrier = Arc::new(Barrier::new(8));
    let mut workers = JoinSet::new();
    for _ in 0..8 {
        let address = server.config.bind;
        let barrier = barrier.clone();
        workers.spawn(async move {
            let mut client = Client::connect(address).await;
            bounded(barrier.wait()).await;
            let pipeline = request(&[b"INCR", b"counter"]).repeat(100);
            client.write(&pipeline).await;
            for _ in 0..100 {
                assert!(matches!(client.frame().await, Frame::Integer(1..=800)));
            }
        });
    }
    while let Some(result) = bounded(workers.join_next()).await {
        result.unwrap();
    }
    let mut client = server.client().await;
    assert_eq!(client.command(&[b"GET", b"counter"]).await, bulk(b"800"));
    server.stop().await;
}

#[tokio::test]
async fn admission_rejects_without_disrupting_existing_client() {
    let server = Server::start(Config {
        max_clients: 1,
        ..Config::default()
    })
    .await;
    let mut first = server.client().await;
    assert_eq!(first.command(&[b"PING"]).await, pong()); // Admission barrier.
    for _ in 0..16 {
        let mut rejected = server.client().await;
        assert_eq!(
            rejected.frame().await,
            Frame::Error("ERR max number of clients reached".into())
        );
        rejected.closed().await;
    }
    assert_eq!(first.command(&[b"PING"]).await, pong());
    assert_eq!(first.command(&[b"QUIT"]).await, ok());
    first.closed().await;
    // EOF follows task/socket teardown, after the permit's release on this runtime.
    let mut replacement = server.client().await;
    assert_eq!(replacement.command(&[b"PING"]).await, pong());
    server.stop().await;
}

#[tokio::test]
async fn auth_and_hello_failures_are_atomic_and_connection_local() {
    let server = Server::start(Config {
        password: Some(b" secret\0 ".to_vec()),
        ..Config::default()
    })
    .await;
    let mut client = server.client().await;
    error_starts(client.command(&[b"PING"]).await, "NOAUTH");
    error_starts(client.command(&[b"AUTH", b"wrong"]).await, "WRONGPASS");
    error_starts(
        client.command(&[b"AUTH", b"someone", b" secret\0 "]).await,
        "WRONGPASS",
    );
    error_starts(
        client
            .command(&[
                b"HELLO",
                b"3",
                b"AUTH",
                b"default",
                b" secret\0 ",
                b"SETNAME",
                b"bad",
            ])
            .await,
        "NOPROTO",
    );
    error_starts(client.command(&[b"PING"]).await, "NOAUTH");
    for suffix in [b"UNKNOWN".as_slice(), b"SETNAME".as_slice()] {
        error_starts(
            client
                .command(&[b"HELLO", b"2", b"AUTH", b"default", b" secret\0 ", suffix])
                .await,
            "ERR",
        );
        error_starts(client.command(&[b"PING"]).await, "NOAUTH");
    }
    error_starts(
        client
            .command(&[
                b"HELLO",
                b"2",
                b"AUTH",
                b"default",
                b" secret\0 ",
                b"SETNAME",
                b"bad name",
            ])
            .await,
        "ERR",
    );
    error_starts(client.command(&[b"PING"]).await, "NOAUTH");
    assert_eq!(client.command(&[b"AUTH", b" secret\0 "]).await, ok());
    assert_eq!(client.command(&[b"CLIENT", b"GETNAME"]).await, Frame::Null);
    assert_eq!(
        client.command(&[b"CLIENT", b"SETNAME", b"original"]).await,
        ok()
    );
    error_starts(
        client
            .command(&[
                b"HELLO", b"3", b"AUTH", b"default", b"bad", b"SETNAME", b"changed",
            ])
            .await,
        "NOPROTO",
    );
    error_starts(
        client
            .command(&[
                b"HELLO", b"2", b"SETNAME", b"changed", b"AUTH", b"default", b"bad",
            ])
            .await,
        "WRONGPASS",
    );
    assert_eq!(client.command(&[b"PING"]).await, pong());
    assert_eq!(
        client.command(&[b"CLIENT", b"GETNAME"]).await,
        bulk(b"original")
    );
    let hello = client
        .command(&[
            b"HELLO",
            b"2",
            b"SETNAME",
            b"new",
            b"AUTH",
            b"default",
            b" secret\0 ",
        ])
        .await;
    let Frame::Array(properties) = hello else {
        panic!("expected RESP2 HELLO array")
    };
    assert!(
        properties
            .as_chunks::<2>()
            .0
            .iter()
            .any(|pair| *pair == [bulk(b"proto"), Frame::Integer(2)])
    );
    assert_eq!(client.command(&[b"CLIENT", b"GETNAME"]).await, bulk(b"new"));
    let mut other = server.client().await;
    error_starts(other.command(&[b"GET", b"key"]).await, "NOAUTH");
    assert_eq!(
        other.command(&[b"AUTH", b"default", b" secret\0 "]).await,
        ok()
    );
    assert_eq!(other.command(&[b"CLIENT", b"GETNAME"]).await, Frame::Null);
    let mut fresh = server.client().await;
    assert!(matches!(
        fresh
            .command(&[
                b"HELLO",
                b"2",
                b"AUTH",
                b"default",
                b" secret\0 ",
                b"SETNAME",
                b"fresh",
            ])
            .await,
        Frame::Array(_)
    ));
    assert_eq!(fresh.command(&[b"PING"]).await, pong());
    assert_eq!(
        fresh.command(&[b"CLIENT", b"GETNAME"]).await,
        bulk(b"fresh")
    );
    server.stop().await;
}

#[tokio::test]
async fn connection_commands_validate_arguments_and_ids_are_unique() {
    let server = Server::start(Config::default()).await;
    let mut first = server.client().await;
    let mut second = server.client().await;
    let id = first.command(&[b"CLIENT", b"ID"]).await;
    assert!(matches!(id, Frame::Integer(1..)));
    assert_ne!(id, second.command(&[b"CLIENT", b"ID"]).await);
    assert_eq!(id, first.command(&[b"CLIENT", b"ID"]).await);
    assert_eq!(first.command(&[b"SELECT", b"0"]).await, ok());
    for args in [
        vec![b"SELECT".as_slice(), b"1"],
        vec![b"SELECT", b"bad"],
        vec![b"SELECT"],
        vec![b"PING", b"one", b"two"],
        vec![b"ECHO"],
        vec![b"QUIT", b"extra"],
        vec![b"CLIENT", b"ID", b"extra"],
        vec![b"CLIENT", b"GETNAME", b"extra"],
        vec![b"CLIENT", b"SETNAME"],
        vec![b"CLIENT", b"SETNAME", b"bad\nname"],
        vec![b"CLIENT", b"SETINFO", b"LIB-NAME"],
        vec![b"CLIENT", b"SETINFO", b"unsupported", b"value"],
        vec![b"CLIENT", b"SETINFO", b"LIB-VER", b"bad value"],
        vec![b"CLIENT", b"NOOP"],
    ] {
        error_starts(first.command(&args).await, "ERR");
    }
    assert_eq!(
        first
            .command(&[b"CLIENT", b"SETINFO", b"LIB-NAME", b"test-client"])
            .await,
        ok()
    );
    assert_eq!(
        first
            .command(&[b"CLIENT", b"SETINFO", b"LIB-VER", b"1.2.3"])
            .await,
        ok()
    );
    assert_eq!(
        first.command(&[b"CLIENT", b"SETNAME", b"named"]).await,
        ok()
    );
    assert_eq!(
        first.command(&[b"CLIENT", b"GETNAME"]).await,
        bulk(b"named")
    );
    assert_eq!(first.command(&[b"CLIENT", b"SETNAME", b""]).await, ok());
    assert_eq!(first.command(&[b"CLIENT", b"GETNAME"]).await, Frame::Null);
    assert!(matches!(first.command(&[b"HELLO"]).await, Frame::Array(_)));
    error_starts(first.command(&[b"AUTH", b"unused"]).await, "ERR");
    server.stop().await;
}

#[tokio::test]
async fn redis_rs_resp2_auth_binary_values_and_pipeline() {
    let server = Server::start(Config {
        password: Some(b"integration-password".to_vec()),
        ..Config::default()
    })
    .await;
    let client = redis::Client::open(format!(
        "redis://default:integration-password@{}/0?protocol=resp2",
        server.config.bind
    ))
    .unwrap();
    let mut connection = bounded(client.get_multiplexed_async_connection())
        .await
        .unwrap();
    let payload = b"\0\xffbinary\r\n";
    bounded(
        redis::cmd("SET")
            .arg("client-key")
            .arg(payload.as_slice())
            .query_async::<()>(&mut connection),
    )
    .await
    .unwrap();
    let result: (Vec<u8>, i64) = bounded(
        redis::pipe()
            .cmd("GET")
            .arg("client-key")
            .cmd("INCR")
            .arg("client-counter")
            .query_async(&mut connection),
    )
    .await
    .unwrap();
    assert_eq!(result, (payload.to_vec(), 1));
    let missing: Option<Vec<u8>> = bounded(
        redis::cmd("GET")
            .arg("missing")
            .query_async(&mut connection),
    )
    .await
    .unwrap();
    assert_eq!(missing, None);
    drop(connection);
    server.stop().await;
}

#[tokio::test]
async fn input_limits_reject_early_and_do_not_execute_following_pipeline() {
    let server = Server::start(Config {
        max_frame_bytes: 128,
        max_value_bytes: 64,
        max_args: 4,
        ..Config::default()
    })
    .await;
    for malformed in [
        b"*1\r\n$65\r\n".as_slice(),
        b"*5\r\n",
        b"PING\r\n",
        b"*0\r\n",
        b"*1\r\n$99999999999999999999999",
        b"*1\r\n$-1\r\n",
        b"*1\r\n*1\r\n$4\r\nPING\r\n",
    ] {
        let mut client = server.client().await;
        client
            .write(&[malformed, &request(&[b"SET", b"should-not-exist", b"bad"])].concat())
            .await;
        error_starts(client.frame().await, "ERR protocol error");
        client.closed().await;
    }
    let mut client = server.client().await;
    assert_eq!(
        client.command(&[b"GET", b"should-not-exist"]).await,
        Frame::Null
    );
    // Total pipeline bytes exceed the per-frame limit; each individual PING fits.
    client.write(&request(&[b"PING"]).repeat(100)).await;
    for _ in 0..100 {
        assert_eq!(client.frame().await, pong());
    }
    server.stop().await;
}

#[tokio::test]
async fn output_limit_is_inclusive_and_stops_following_mutations() {
    let server = Server::start(Config {
        max_output_bytes: 64,
        ..Config::default()
    })
    .await;
    let mut client = server.client().await;
    assert_eq!(client.command(&[b"SET", b"fits", &[b'x'; 57]]).await, ok());
    assert_eq!(client.command(&[b"GET", b"fits"]).await, bulk(&[b'x'; 57]));
    assert_eq!(client.command(&[b"SET", b"large", &[b'x'; 58]]).await, ok());
    client
        .write(
            &[
                request(&[b"GET", b"large"]),
                request(&[b"INCR", b"not-executed"]),
            ]
            .concat(),
        )
        .await;
    client.closed().await;
    let mut observer = server.client().await;
    assert_eq!(
        observer.command(&[b"GET", b"not-executed"]).await,
        Frame::Null
    );
    // Repeated shared values must be charged by encoded response size, not stored size.
    observer.write(&request(&[b"MGET", b"fits", b"fits"])).await;
    observer.closed().await;
    server.stop().await;
}

#[tokio::test]
async fn frame_limit_is_inclusive_even_with_more_pipeline_bytes() {
    let server = Server::start(Config {
        max_frame_bytes: 64,
        max_value_bytes: 64,
        ..Config::default()
    })
    .await;
    let mut client = server.client().await;
    let fits = request(&[b"ECHO", &[b'x'; 43]]);
    assert_eq!(fits.len(), 64);
    client.write(&[fits, request(&[b"PING"])].concat()).await;
    assert_eq!(client.frame().await, bulk(&[b'x'; 43]));
    assert_eq!(client.frame().await, pong());
    client.write(&request(&[b"ECHO", &[b'x'; 44]])).await;
    error_starts(client.frame().await, "ERR protocol error");
    client.closed().await;
    server.stop().await;
}

#[tokio::test]
async fn partial_request_timeout_is_isolated() {
    let server = Server::start(Config {
        io_timeout_secs: 1,
        ..Config::default()
    })
    .await;
    let mut slow = server.client().await;
    assert_eq!(slow.command(&[b"PING"]).await, pong());
    slow.write(b"*2\r\n$4\r\nECHO\r\n$4\r\nxy").await;
    slow.closed().await;
    let mut healthy = server.client().await;
    assert_eq!(healthy.command(&[b"PING"]).await, pong());
    server.stop().await;
}

#[tokio::test]
async fn shutdown_cancels_idle_and_partial_reads_before_returning() {
    let server = Server::start(Config {
        io_timeout_secs: 60,
        ..Config::default()
    })
    .await;
    let mut idle = server.client().await;
    let mut partial = server.client().await;
    assert_eq!(idle.command(&[b"PING"]).await, pong());
    assert_eq!(partial.command(&[b"PING"]).await, pong());
    partial.write(b"*1\r\n$20\r\nunfinished").await;
    bounded(server.stop()).await;
    idle.closed().await;
    partial.closed().await;
}

#[tokio::test]
async fn shutdown_interrupts_backpressure_without_holding_the_database_lock() {
    let server = Server::start(Config {
        io_timeout_secs: 60,
        max_output_bytes: 64 * 1024 * 1024,
        ..Config::default()
    })
    .await;
    let mut blocked = server.client().await;
    let value = vec![b'x'; 1024 * 1024];
    assert_eq!(blocked.command(&[b"SET", b"blob", &value]).await, ok());
    let mut args: Vec<&[u8]> = vec![b"MGET"];
    args.extend(std::iter::repeat_n(b"blob".as_slice(), 32));
    blocked.write(&request(&args)).await;
    // Receipt of the header proves response construction/write has begun. Stop
    // reading the 32 MiB body so ordinary TCP flow control applies backpressure.
    assert_eq!(blocked.line().await, b"*32");
    let mut observer = server.client().await;
    assert_eq!(observer.command(&[b"PING"]).await, pong());
    server.stop().await;
    bounded(async {
        let mut scratch = [0; 8192];
        loop {
            match blocked.0.read(&mut scratch).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => break,
                Err(error) => panic!("unexpected drain failure: {error}"),
            }
        }
    })
    .await;
    observer.closed().await;
}

#[tokio::test]
async fn invalid_config_is_rejected_before_serving() {
    let listener = bounded(TcpListener::bind("127.0.0.1:0")).await.unwrap();
    let error = bounded(serve(
        listener,
        Config {
            max_clients: usize::MAX,
            ..Config::default()
        },
        std::future::pending(),
    ))
    .await
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn config_rejects_zero_overflowing_and_inconsistent_limits() {
    let invalid: &[fn(&mut Config)] = &[
        |c| c.max_memory = 0,
        |c| c.max_memory = usize::MAX,
        |c| c.max_clients = 0,
        |c| c.max_clients = usize::MAX,
        |c| c.max_frame_bytes = 0,
        |c| c.max_frame_bytes = usize::MAX,
        |c| c.max_value_bytes = 0,
        |c| c.max_value_bytes = c.max_frame_bytes + 1,
        |c| c.max_args = 0,
        |c| c.max_args = usize::MAX,
        |c| c.max_output_bytes = 0,
        |c| c.max_output_bytes = usize::MAX,
        |c| c.io_timeout_secs = 0,
        |c| c.io_timeout_secs = u64::MAX,
        |c| c.shutdown_timeout_secs = 0,
        |c| c.shutdown_timeout_secs = u64::MAX,
        |c| c.password = Some(Vec::new()),
        |c| c.password = Some(vec![b'x'; 4097]),
    ];
    Config::default().validate().unwrap();
    for change in invalid {
        let mut config = Config::default();
        change(&mut config);
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

struct PasswordFile(PathBuf);

impl PasswordFile {
    fn new(contents: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "remik-password-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).unwrap();
        let guard = Self(path);
        file.write_all(contents).unwrap();
        guard
    }
}

impl Drop for PasswordFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            assert!(
                std::thread::panicking(),
                "cannot clean up password fixture: {error}"
            );
        }
    }
}

#[test]
fn password_files_preserve_bytes_and_trim_only_one_line_ending() {
    for (file, expected) in [
        (b" secret \t\r\n".as_slice(), b" secret \t".as_slice()),
        (b" secret \n", b" secret "),
        (b"secret\n\n", b"secret\n"),
        (b"\xff\0\r", b"\xff\0\r"),
        (b" \t", b" \t"),
    ] {
        let file = PasswordFile::new(file);
        let mut config = Config::default();
        config.load_password_file(&file.0).unwrap();
        assert_eq!(config.password.as_deref(), Some(expected));
    }
    for contents in [
        Vec::new(),
        b"\n".to_vec(),
        b"\r\n".to_vec(),
        vec![b'x'; 4097],
        vec![b'x'; 4099],
    ] {
        let file = PasswordFile::new(&contents);
        let mut config = Config {
            password: Some(b"unchanged".to_vec()),
            ..Config::default()
        };
        assert_eq!(
            config.load_password_file(&file.0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(config.password.as_deref(), Some(b"unchanged".as_slice()));
    }
    let maximum = PasswordFile::new(&[vec![b'x'; 4096], b"\r\n".to_vec()].concat());
    Config::default().load_password_file(&maximum.0).unwrap();
    let mut config = Config::default();
    assert_eq!(
        config
            .load_password_file(&std::env::temp_dir())
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Both calls are best effort during assertion unwinding; always reap.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_remik"));
    command
        .env_remove("REMIK_BIND")
        .env_remove("REMIK_PASSWORD_FILE")
        .env_remove("RUST_LOG");
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

async fn child_status(child: &mut ChildGuard) -> ExitStatus {
    bounded(async {
        let mut poll = tokio::time::interval(Duration::from_millis(10));
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                return status;
            }
            poll.tick().await;
        }
    })
    .await
}

#[tokio::test]
async fn cli_version_and_healthcheck_run_the_built_binary() {
    let mut version = ChildGuard(cli().arg("--version").spawn().unwrap());
    assert!(child_status(&mut version).await.success());
    let mut output = String::new();
    version
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    assert_eq!(
        output.trim(),
        format!("remik {}", env!("CARGO_PKG_VERSION"))
    );

    let password = PasswordFile::new(b" binary\0password \r\n");
    let mut config = Config::default();
    config.load_password_file(&password.0).unwrap();
    let server = Server::start(config).await;
    bounded(healthcheck(&server.config)).await.unwrap();
    let wildcard = format!("0.0.0.0:{}", server.config.bind.port());
    for args in [
        vec!["healthcheck", "--bind", &wildcard],
        vec!["--bind", &wildcard, "healthcheck"],
        vec!["healthcheck"],
    ] {
        let mut child = ChildGuard(
            cli()
                .args(args)
                .env("REMIK_BIND", &wildcard)
                .env("REMIK_PASSWORD_FILE", &password.0)
                .spawn()
                .unwrap(),
        );
        assert!(child_status(&mut child).await.success());
    }
    let wrong = PasswordFile::new(b"wrong-test-password\n");
    let mut child = ChildGuard(
        cli()
            .args(["healthcheck", "--bind", &wildcard])
            .arg("--password-file")
            .arg(&wrong.0)
            .spawn()
            .unwrap(),
    );
    assert!(!child_status(&mut child).await.success());
    let mut stderr = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stderr.contains("wrong-test-password"));
    server.stop().await;

    let mut child = ChildGuard(cli().args(["--max-memory", "0"]).spawn().unwrap());
    assert!(!child_status(&mut child).await.success());
}
