use std::net::{Ipv4Addr, SocketAddr};

use axum::Router;
use tokio::{net::TcpListener, signal};
use tonic::transport::server::Routes;

pub async fn serve(app: Router, grpc: Routes, port: u16) -> anyhow::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let listener = TcpListener::bind(addr).await.unwrap();

    Ok(axum::serve(
        listener,
        crate::hybrid::hybrid(app.into_make_service(), grpc),
    )
    .with_graceful_shutdown(shutdown_signal())
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
