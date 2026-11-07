//! Server startup and graceful shutdown.

use std::{net::SocketAddr, path::Path, time::Duration};

use gsd_core::{
    auth::{AuthError, AuthService},
    store::{self, StoreError},
};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{config::Config, http, state::AppState};

const HOUSEKEEPING_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("could not prepare data directory {path}")]
    DataDir {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("could not listen on {addr}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("the HTTP server failed")]
    Serve(#[source] std::io::Error),
}

/// Opens storage, builds the app, serves HTTP until Ctrl+C / SIGTERM, then shuts down cleanly.
pub async fn run(config: Config) -> Result<(), ServeError> {
    prepare_data_dir(&config.data_dir)?;
    let pool = store::open(&config.database_path()).await?;
    let state = AppState::build(config, pool.clone()).await?;

    let addr = state.config.bind;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServeError::Bind { addr, source })?;
    announce(&state, &listener);

    let housekeeping = spawn_housekeeping(state.auth.clone());
    let app = http::router(state).into_make_service_with_connect_info::<SocketAddr>();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    housekeeping.abort();
    pool.close().await;
    tracing::info!("shut down cleanly");
    served.map_err(ServeError::Serve)
}

/// Creates the data directory, readable only by the app (it holds secrets and backups).
fn prepare_data_dir(path: &Path) -> Result<(), ServeError> {
    let map_err = |source| ServeError::DataDir {
        path: path.to_owned(),
        source,
    };
    std::fs::create_dir_all(path).map_err(map_err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(map_err)?;
    }
    Ok(())
}

fn announce(state: &AppState, listener: &TcpListener) {
    let address = listener
        .local_addr()
        .map_or_else(|_| state.config.bind.to_string(), |a| a.to_string());
    tracing::info!(
        data_dir = %state.config.data_dir.display(),
        "{} {} is listening on http://{address}",
        gsd_brand::NAME,
        gsd_brand::VERSION
    );
    if let Some(token) = state.setup_gate.token() {
        tracing::warn!(
            "first-run setup is open to the network; enter this setup token to create the owner: {token}"
        );
    }
}

/// Periodic maintenance: purge expired sessions.
fn spawn_housekeeping(auth: AuthService) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(HOUSEKEEPING_INTERVAL);
        loop {
            ticker.tick().await;
            match auth.purge_expired_sessions().await {
                Ok(0) => {}
                Ok(removed) => tracing::debug!(removed, "purged expired sessions"),
                Err(error) => tracing::warn!(%error, "failed to purge expired sessions"),
            }
        }
    })
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
