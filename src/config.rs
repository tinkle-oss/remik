//! Validated resource limits and password-file loading for native servers.

use std::{
    fs::File,
    io::{self, Read},
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    time::Duration,
};

use clap::Args;
use tokio::sync::Semaphore;

use crate::protocol::Limits;

pub(crate) const MAX_PASSWORD_BYTES: usize = 4096;

/// Server settings. Secrets deliberately do not implement `Debug`.
///
/// `max_memory` bounds database accounting, not total process memory. Network
/// buffers and connection metadata have separate per-client bounds.
#[derive(Clone, Args)]
pub struct Config {
    /// Listen address (numeric IP and port).
    #[arg(
        long,
        env = "REMIK_BIND",
        default_value = "127.0.0.1:6379",
        global = true
    )]
    pub bind: SocketAddr,
    /// Database memory budget in integer bytes (1..=1099511627776).
    #[arg(long, default_value_t = 67_108_864, global = true)]
    pub max_memory: usize,
    /// Concurrent clients (1..=4096); 128 limits default buffer exposure.
    #[arg(long, default_value_t = 128, global = true)]
    pub max_clients: usize,
    /// Maximum request frame in bytes (64..=67108864).
    #[arg(long, default_value_t = 8_388_608, global = true)]
    pub max_frame_bytes: usize,
    /// Maximum bulk argument in bytes (1..=16777216, at most the frame limit).
    #[arg(long, default_value_t = 1_048_576, global = true)]
    pub max_value_bytes: usize,
    /// Maximum arguments per command (1..=65536).
    #[arg(long, default_value_t = 1024, global = true)]
    pub max_args: usize,
    /// Maximum encoded response in bytes (64..=67108864).
    #[arg(long, default_value_t = 16_777_216, global = true)]
    pub max_output_bytes: usize,
    /// Idle/partial-request and whole-response I/O deadline (1..=3600 seconds).
    #[arg(long, default_value_t = 30, global = true)]
    pub io_timeout_secs: u64,
    /// Cooperative shutdown deadline before aborting connections (1..=60 seconds).
    #[arg(long, default_value_t = 5, global = true)]
    pub shutdown_timeout_secs: u64,
    /// Optional binary password, supplied by an embedder or a bounded file.
    /// This field is never accepted as a command-line argument.
    #[arg(skip)]
    pub password: Option<Vec<u8>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 6379)),
            max_memory: 67_108_864,
            max_clients: 128,
            max_frame_bytes: 8_388_608,
            max_value_bytes: 1_048_576,
            max_args: 1024,
            max_output_bytes: 16_777_216,
            io_timeout_secs: 30,
            shutdown_timeout_secs: 5,
            password: None,
        }
    }
}

impl Config {
    /// Validate both CLI and programmatically constructed settings before use.
    pub fn validate(&self) -> io::Result<()> {
        if self.max_memory == 0 || self.max_memory as u128 > (1_u128 << 40) {
            return Err(invalid("max-memory must be in 1..=1099511627776"));
        }
        if !(1..=4096).contains(&self.max_clients) || self.max_clients > Semaphore::MAX_PERMITS {
            return Err(invalid(
                "max-clients must be in 1..=4096 and fit the semaphore",
            ));
        }
        if !(64..=67_108_864).contains(&self.max_frame_bytes) {
            return Err(invalid("max-frame-bytes must be in 64..=67108864"));
        }
        if !(1..=16_777_216).contains(&self.max_value_bytes)
            || self.max_value_bytes > self.max_frame_bytes
        {
            return Err(invalid(
                "max-value-bytes must be in 1..=16777216 and not exceed max-frame-bytes",
            ));
        }
        if !(1..=65_536).contains(&self.max_args) {
            return Err(invalid("max-args must be in 1..=65536"));
        }
        if !(64..=67_108_864).contains(&self.max_output_bytes) {
            return Err(invalid("max-output-bytes must be in 64..=67108864"));
        }
        if !(1..=3600).contains(&self.io_timeout_secs) {
            return Err(invalid("io-timeout-secs must be in 1..=3600"));
        }
        if !(1..=60).contains(&self.shutdown_timeout_secs) {
            return Err(invalid("shutdown-timeout-secs must be in 1..=60"));
        }
        if let Some(password) = &self.password {
            if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
                return Err(invalid("password must contain 1..=4096 bytes"));
            }
            let auth_bytes = password.len() + password.len().to_string().len() + 19;
            if self.max_value_bytes < 4
                || password.len() > self.max_value_bytes
                || auth_bytes > self.max_frame_bytes
                || self.max_args < 2
            {
                return Err(invalid(
                    "request limits must accommodate AUTH and the configured password",
                ));
            }
        }
        Ok(())
    }

    /// Load at most 4098 bytes from a regular file, removing exactly one terminal
    /// CRLF or LF. Other whitespace and binary bytes are part of the password.
    /// A rejected file leaves the previous password unchanged.
    pub fn load_password_file(&mut self, path: &Path) -> io::Result<()> {
        let metadata =
            std::fs::metadata(path).map_err(|_| invalid("cannot inspect password file"))?;
        if !metadata.is_file() || metadata.len() > (MAX_PASSWORD_BYTES + 2) as u64 {
            return Err(invalid(
                "password file must be a regular file of at most 4098 bytes",
            ));
        }
        let file = File::open(path).map_err(|_| invalid("cannot open password file"))?;
        if !file
            .metadata()
            .map_err(|_| invalid("cannot inspect opened password file"))?
            .is_file()
        {
            return Err(invalid("password file must be a regular file"));
        }
        let mut password = Vec::new();
        file.take((MAX_PASSWORD_BYTES + 3) as u64)
            .read_to_end(&mut password)
            .map_err(|_| invalid("cannot read password file"))?;
        if password.ends_with(b"\r\n") {
            password.truncate(password.len() - 2);
        } else if password.ends_with(b"\n") {
            password.truncate(password.len() - 1);
        }
        if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
            return Err(invalid("password must contain 1..=4096 bytes"));
        }
        // Validate without cloning the existing secret or changing it on failure.
        let previous = self.password.replace(password);
        if let Err(error) = self.validate() {
            self.password = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn limits(&self) -> Limits {
        Limits {
            max_frame_bytes: self.max_frame_bytes,
            max_bulk_bytes: self.max_value_bytes,
            max_args: self.max_args,
        }
    }

    pub(crate) fn io_timeout(&self) -> Duration {
        Duration::from_secs(self.io_timeout_secs)
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
