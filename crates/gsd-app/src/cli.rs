//! Command-line interface: `serve` (default), `reset-password`, `healthcheck`, `brand`.

use std::{net::SocketAddr, time::Duration};

use clap::{Parser, Subcommand, ValueEnum};
use gsd_core::{
    audit::{AuditEntry, AuditLog, actions},
    auth::{AuthError, AuthService},
    store::{self, StoreError},
    supervisor::{HostSandbox, SandboxSupport, sandbox::cgroup::CgroupsUnavailable},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing_subscriber::EnvFilter;

use crate::{
    config::{Config, ConfigError, ConfigOverrides},
    serve::{self, ServeError},
};

#[derive(Debug, Parser)]
#[command(name = gsd_brand::SLUG, version = gsd_brand::VERSION, about = gsd_brand::NAME)]
pub struct Cli {
    #[command(flatten)]
    pub config: ConfigOverrides,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the web server (the default when no command is given).
    Serve,
    /// Give a user a temporary password (they must change it at next sign-in) and sign them out everywhere.
    ResetPassword { username: String },
    /// Exit successfully if the local server answers its health check (used by Docker).
    Healthcheck,
    /// Print one branding value (used by the Docker build).
    Brand {
        #[arg(value_enum)]
        field: BrandField,
    },
    /// Test the per-server sandbox on this host; exits non-zero if an enforced check fails.
    IsolationCheck,
    /// Internal: one sandboxed probe action (used by `isolation-check`).
    #[command(hide = true)]
    IsolationProbe { action: String, target: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BrandField {
    Name,
    Slug,
    Port,
    EnvPrefix,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Serve(#[from] ServeError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("health check failed: {0}")]
    Unhealthy(String),
    #[error("isolation check failed: {0}")]
    Isolation(String),
}

/// Logging filter from `<PREFIX>_LOG` (same syntax as `RUST_LOG`), defaulting to `info`.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_env(gsd_brand::env_var("LOG"))
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

pub async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve::run(load_config(&cli.config)?).await?,
        Command::ResetPassword { username } => reset_password(&load_config(&cli.config)?, &username).await?,
        Command::Healthcheck => healthcheck(&load_config(&cli.config)?).await?,
        Command::Brand { field } => println!("{}", brand_value(field)),
        Command::IsolationCheck => isolation_check(&load_config(&cli.config)?).await?,
        Command::IsolationProbe { action, target } => {
            std::process::exit(gsd_core::supervisor::sandbox::check::probe_main(&action, &target))
        }
    }
    Ok(())
}

/// Prints a pass/fail table of the sandbox checks, run inside the data directory (its
/// filesystem and parent permissions are what servers will get).
async fn isolation_check(config: &Config) -> Result<(), CliError> {
    let fail = |error: std::io::Error| CliError::Isolation(error.to_string());
    let probe = std::env::current_exe().map_err(fail)?;
    let scratch =
        gsd_core::supervisor::sandbox::check::scratch_in(&config.sandbox_check_dir()).map_err(fail)?;
    println!("Testing in {}", config.data_dir.display());
    // Hard limits need the app's own control group; only the running app can test them
    // (POST /system/isolation-check).
    let host = HostSandbox::new(SandboxSupport::detect(), None)
        .with_cgroups(Err(CgroupsUnavailable(
            "tested only by the running app (its isolation check)".into(),
        )))
        .with_this_binary_as_launcher();
    let report = gsd_core::supervisor::sandbox::check::run(&probe, scratch.path(), &host)
        .await
        .map_err(fail)?;

    println!(
        "Isolation level: {:?} (Landlock ABI: {:?})",
        report.level, report.support.landlock_abi
    );
    for check in &report.checks {
        let mark = match (check.enforced, check.passed) {
            (true, true) => "PASS",
            (true, false) => "FAIL",
            (false, _) => "n/a ",
        };
        println!("  [{mark}] {}: {}", check.name, check.detail);
    }
    if report.ok() {
        Ok(())
    } else {
        Err(CliError::Isolation("an enforced check failed".into()))
    }
}

fn load_config(overrides: &ConfigOverrides) -> Result<Config, ConfigError> {
    Config::load(overrides, |name| std::env::var(name).ok())
}

async fn reset_password(config: &Config, username: &str) -> Result<(), CliError> {
    let pool = store::open(&config.database_path()).await?;
    let auth = AuthService::new(pool.clone(), config.session_ttl);
    let temporary = auth.reset_password(username).await?;
    AuditLog::new(pool.clone())
        .record(
            AuditEntry::anonymous(actions::PASSWORD_RESET)
                .with_details(serde_json::json!({ "username": username, "via": "cli" })),
        )
        .await;
    pool.close().await;

    println!("Temporary password for {username}: {temporary}");
    println!("They will be asked to choose a new password after signing in.");
    Ok(())
}

/// A minimal HTTP/1.0 request to `/api/v1/health`, so the Docker image needs no curl.
async fn healthcheck(config: &Config) -> Result<(), CliError> {
    let target = loopback_for(config.bind);
    let check = async {
        let mut stream = tokio::net::TcpStream::connect(target).await?;
        stream
            .write_all(b"GET /api/v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        Ok::<_, std::io::Error>(response)
    };

    let response = tokio::time::timeout(Duration::from_secs(5), check)
        .await
        .map_err(|_| CliError::Unhealthy(format!("no answer from {target} within 5s")))?
        .map_err(|error| CliError::Unhealthy(format!("{target}: {error}")))?;

    let status_line = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    if status_line.split_whitespace().nth(1) == Some("200") {
        Ok(())
    } else {
        Err(CliError::Unhealthy(format!(
            "unexpected response: {status_line:?}"
        )))
    }
}

/// `0.0.0.0:p` / `[::]:p` are not connectable addresses; probe loopback instead.
fn loopback_for(bind: SocketAddr) -> SocketAddr {
    let mut target = bind;
    if bind.ip().is_unspecified() {
        let loopback = if bind.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        };
        target.set_ip(loopback);
    }
    target
}

fn brand_value(field: BrandField) -> String {
    match field {
        BrandField::Name => gsd_brand::NAME.to_owned(),
        BrandField::Slug => gsd_brand::SLUG.to_owned(),
        BrandField::Port => gsd_brand::DEFAULT_PORT.to_string(),
        BrandField::EnvPrefix => gsd_brand::ENV_PREFIX.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthcheck_probes_loopback_for_wildcard_binds() {
        assert_eq!(
            loopback_for("0.0.0.0:80".parse().unwrap()),
            "127.0.0.1:80".parse().unwrap()
        );
        assert_eq!(
            loopback_for("[::]:80".parse().unwrap()),
            "[::1]:80".parse().unwrap()
        );
        assert_eq!(
            loopback_for("10.0.0.5:80".parse().unwrap()),
            "10.0.0.5:80".parse().unwrap()
        );
    }

    #[test]
    fn cli_parses_flags_and_commands() {
        let cli =
            Cli::try_parse_from(["app", "--bind", "127.0.0.1:9000", "reset-password", "steve"]).unwrap();
        assert!(matches!(cli.command, Some(Command::ResetPassword { ref username }) if username == "steve"));
        assert_eq!(cli.config.bind.unwrap().port(), 9000);
    }
}
