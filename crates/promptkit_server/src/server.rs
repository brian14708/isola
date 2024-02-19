use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use axum::Router;
use axum_server::Handle;
use tokio::{signal, time::sleep};
use tracing::info;

pub async fn serve(app: Router, port: u16) -> anyhow::Result<()> {
    let handle = Handle::new();
    tokio::spawn(graceful_shutdown(handle.clone()));

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    Ok(axum_server::bind(addr)
        .handle(handle)
        .serve(app.into_make_service())
        .await?)
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
        () = ctrl_c => {},
        () = terminate => {},
    }
}

async fn graceful_shutdown(handle: Handle) {
    shutdown_signal().await;

    info!("Shutting down...");
    handle.graceful_shutdown(Some(Duration::from_secs(30)));
    loop {
        sleep(Duration::from_secs(1)).await;
        info!("Alive connections: {}", handle.connection_count());
    }
}
