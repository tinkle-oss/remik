use std::{future::Future, io, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use remik::{config::Config, server};
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(flatten)]
    config: Config,
    /// Read a binary password from a regular file (at most 4098 bytes).
    #[arg(long, env = "REMIK_PASSWORD_FILE", global = true)]
    password_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate if configured, then require PONG from the configured address.
    Healthcheck,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("remik=info"));
    if tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .try_init()
        .is_err()
    {
        eprintln!("remik: cannot initialize tracing");
        return ExitCode::FAILURE;
    }
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "remik failed");
            ExitCode::FAILURE
        }
    }
}

async fn run(mut cli: Cli) -> io::Result<()> {
    cli.config.validate()?;
    if let Some(path) = &cli.password_file {
        cli.config.load_password_file(path)?;
    }
    if matches!(cli.command, Some(Command::Healthcheck)) {
        return server::healthcheck(&cli.config).await;
    }
    let shutdown = shutdown_signal()?;
    let listener = TcpListener::bind(cli.config.bind).await?;
    tracing::info!(address = %listener.local_addr()?, "listening");
    let mut signal_result = Ok(());
    server::serve(listener, cli.config, async {
        signal_result = shutdown.await;
    })
    .await?;
    signal_result
}

// Install Unix handlers before publishing listener readiness.
fn shutdown_signal() -> io::Result<impl Future<Output = io::Result<()>> + Send> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
            Ok(())
        })
    }
    #[cfg(not(unix))]
    {
        Ok(tokio::signal::ctrl_c())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_consistent() {
        Cli::command().debug_assert();
    }
}
