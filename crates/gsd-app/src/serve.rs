//! Server startup and graceful shutdown.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{TimeDelta, Utc};
use gsd_core::{
    store::{self, StoreError},
    trash::{self, Purged},
};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    config::Config,
    http,
    state::{AppState, BuildError},
};

const HOUSEKEEPING_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long running game servers get to stop when the app shuts down.
const SERVER_STOP_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("could not prepare data directory {path}")]
    DataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("could not listen on {addr}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("the HTTP server failed")]
    Serve(#[source] std::io::Error),
}

/// Opens storage, builds the app, serves HTTP until Ctrl+C / SIGTERM, then shuts down cleanly:
/// close live connections, finish HTTP requests, stop game servers gracefully, close the database.
pub async fn run(config: Config) -> Result<(), ServeError> {
    prepare_data_dir(&config)?;
    let pool = store::open(&config.database_path()).await?;
    let state = AppState::build(config, pool.clone()).await?;

    let addr = state.config.bind;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServeError::Bind { addr, source })?;
    announce(&state, &listener);

    let housekeeping = spawn_housekeeping(state.clone());
    // Before auto-start, so "when the server starts" schedules see those starts. It stops on
    // the shutdown token, before the servers are stopped below.
    let scheduler = state.scheduler.start(state.shutdown.clone());
    state.instances.start_auto_start_instances().await;

    let shutdown = state.shutdown.clone();
    let instances = state.instances.clone();
    let app = http::router(state).into_make_service_with_connect_info::<SocketAddr>();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown.cancel();
        })
        .await;

    let _ = scheduler.await;
    tracing::info!("stopping game servers");
    instances.stop_all(SERVER_STOP_TIMEOUT).await;
    housekeeping.abort();
    pool.close().await;
    tracing::info!("shut down cleanly");
    served.map_err(ServeError::Serve)
}

/// Creates the data directory tree with the permissions the sandbox relies on:
/// the data dir and `instances/` are traversable but not listable by other users (0711);
/// Java runtimes are world-readable (0755); everything else stays private.
fn prepare_data_dir(config: &Config) -> Result<(), ServeError> {
    let runtimes_parent = config
        .runtimes_dir()
        .parent()
        .map_or_else(|| config.runtimes_dir(), Path::to_path_buf);
    let dirs = [
        (config.data_dir.clone(), 0o711),
        (config.instances_dir(), 0o711),
        (runtimes_parent, 0o755),
        (config.runtimes_dir(), 0o755),
        (config.trash_dir(), 0o700),
        (config.data_dir.join("cache"), 0o700),
        (config.sandbox_check_dir(), 0o711),
        (config.imports_dir(), 0o700),
    ];
    for (path, mode) in dirs {
        let map_err = |source| ServeError::DataDir {
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(&path).map_err(map_err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).map_err(map_err)?;
        }
        #[cfg(not(unix))]
        let _ = mode;
    }
    Ok(())
}

fn announce(state: &AppState, listener: &TcpListener) {
    let address = listener
        .local_addr()
        .map_or_else(|_| state.config.bind.to_string(), |a| a.to_string());
    let support = state.instances.supervisors().host().support();
    tracing::info!(
        data_dir = %state.config.data_dir.display(),
        isolation = ?support.level(),
        "{} {} is listening on http://{address}",
        gsd_brand::NAME,
        gsd_brand::VERSION
    );
    let host = state.instances.supervisors().host();
    if let Some(problem) = host.problem() {
        tracing::error!("{problem}");
    }
    match host.cgroups_problem() {
        None => tracing::info!("servers get hard memory and task limits (cgroup v2)"),
        Some(reason) => tracing::info!("hard memory limits are off: {reason}"),
    }
    if let Some(token) = state.setup_gate.token() {
        tracing::warn!(
            "first-run setup is open to the network; enter this setup token to create the owner: {token}"
        );
    }
}

/// Periodic maintenance: purge expired sessions, empty old trash.
fn spawn_housekeeping(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(HOUSEKEEPING_INTERVAL);
        loop {
            ticker.tick().await;
            match state.auth.purge_expired_sessions().await {
                Ok(0) => {}
                Ok(removed) => tracing::debug!(removed, "purged expired sessions"),
                Err(error) => tracing::warn!(%error, "failed to purge expired sessions"),
            }
            empty_trash(&state).await;
        }
    })
}

/// Removes what has been in a trash folder (the app's, and each server's) longer than the
/// configured retention.
pub async fn empty_trash(state: &AppState) -> Purged {
    let days = state.config.trash_retention_days;
    if days == 0 {
        return Purged::default();
    }
    let cutoff = Utc::now() - TimeDelta::days(i64::from(days));
    let mut folders = vec![state.config.trash_dir()];
    for instance in state.instances.registry().list().await {
        folders.push(instance.layout.trash_dir());
    }
    let purged = tokio::task::spawn_blocking(move || {
        let mut total = Purged::default();
        for folder in folders {
            match trash::purge(&folder, cutoff) {
                Ok(purged) => total += purged,
                Err(error) => {
                    tracing::warn!(%error, folder = %folder.display(), "could not empty a trash folder");
                }
            }
        }
        total
    })
    .await
    .unwrap_or_default();
    if purged.removed > 0 {
        tracing::info!(
            removed = purged.removed,
            days,
            "emptied old entries from the trash"
        );
    }
    purged
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "could not listen for Ctrl+C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "could not listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown requested");
}
