use std::net::{Ipv6Addr, SocketAddr};

use axum::Router;
use tokio::{net::TcpListener, signal};
use tracing::info;

pub async fn serve(app: Router, port: u16) -> anyhow::Result<()> {
    let addr = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));
    info!(%addr, "Binding isola-server listener");
    let listener = TcpListener::bind(addr).await?;
    if let Ok(local_addr) = listener.local_addr() {
        info!(%local_addr, "isola-server listener ready");
    } else {
        info!(%addr, "isola-server listener ready");
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("isola-server shutdown complete");
    Ok(())
}

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
        () = ctrl_c => info!("Received Ctrl+C shutdown signal"),
        () = terminate => info!("Received SIGTERM shutdown signal"),
    }
}
