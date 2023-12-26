use std::time::Duration;

use axum_server::Handle;
use tokio::{signal, time::sleep};
use tracing::info;

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

pub async fn graceful_shutdown(handle: Handle) {
    shutdown_signal().await;

    info!("shutting down...");
    handle.graceful_shutdown(Some(Duration::from_secs(30)));
    loop {
        sleep(Duration::from_secs(1)).await;
        info!("alive connections: {}", handle.connection_count());
    }
}
