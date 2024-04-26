use std::net::{Ipv4Addr, SocketAddr};

use axum::Router;
use opentelemetry::global;
use tokio::{net::TcpListener, signal};

pub async fn serve<
    Grpc: tower::Service<
            http_02::Request<tonic::transport::Body>,
            Response = http_02::Response<GrpcBody>,
        > + Send
        + Clone
        + 'static,
    GrpcBody: http_body_04::Body<Data = bytes::Bytes, Error = tonic::Status> + Send + 'static,
>(
    app: Router,
    grpc: Grpc,
    port: u16,
) -> anyhow::Result<()>
where
    <Grpc as tower::Service<http_02::Request<tonic::transport::Body>>>::Future: std::marker::Send,
{
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let listener = TcpListener::bind(addr).await.unwrap();

    axum::serve(
        listener,
        crate::hybrid::hybrid(app.into_make_service(), grpc),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    global::shutdown_tracer_provider();
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
        () = ctrl_c => {},
        () = terminate => {},
    }
}
