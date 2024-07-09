use http_body_util::BodyExt;
use std::{future::Future, pin::Pin, task::Poll};

pub fn hybrid<MakeWeb, Grpc>(make_web: MakeWeb, grpc: Grpc) -> HybridMakeService<MakeWeb, Grpc> {
    HybridMakeService { make_web, grpc }
}

pub struct HybridMakeService<MakeWeb, Grpc> {
    make_web: MakeWeb,
    grpc: Grpc,
}

impl<ConnInfo, MakeWeb, Grpc> tower::Service<ConnInfo> for HybridMakeService<MakeWeb, Grpc>
where
    MakeWeb: tower::Service<ConnInfo>,
    Grpc: Clone,
{
    type Response = HybridService<MakeWeb::Response, Grpc>;
    type Error = MakeWeb::Error;
    type Future = HybridMakeServiceFuture<MakeWeb::Future, Grpc>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.make_web.poll_ready(cx)
    }

    fn call(&mut self, conn_info: ConnInfo) -> Self::Future {
        HybridMakeServiceFuture {
            web_future: self.make_web.call(conn_info),
            grpc: Some(self.grpc.clone()),
        }
    }
}

#[pin_project::pin_project]
pub struct HybridMakeServiceFuture<WebFuture, Grpc> {
    #[pin]
    web_future: WebFuture,
    grpc: Option<Grpc>,
}

impl<WebFuture, WebError, Web, Grpc> Future for HybridMakeServiceFuture<WebFuture, Grpc>
where
    WebFuture: Future<Output = Result<Web, WebError>>,
{
    type Output = Result<HybridService<Web, Grpc>, WebError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context) -> Poll<Self::Output> {
        let this = self.project();
        match this.web_future.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(web)) => Poll::Ready(Ok(HybridService {
                web,
                grpc: this.grpc.take().expect("Cannot poll twice!"),
            })),
        }
    }
}

#[derive(Clone)]
pub struct HybridService<Web, Grpc> {
    web: Web,
    grpc: Grpc,
}

impl<Web, Grpc, WebBody, GrpcBody> tower::Service<axum::http::Request<WebBody>>
    for HybridService<Web, Grpc>
where
    Web: tower::Service<axum::http::Request<WebBody>, Response = axum::response::Response>,
    Grpc: tower::Service<http::Request<tonic::body::BoxBody>, Response = http::Response<GrpcBody>>,
    WebBody: axum::body::HttpBody<Data = bytes::Bytes> + Send + 'static,
    WebBody::Error: Into<axum::BoxError>,
    GrpcBody: http_body::Body<Data = bytes::Bytes, Error = tonic::Status> + Send + 'static,
{
    type Response = Web::Response;
    type Error = Web::Error;
    type Future = HybridFuture<Web::Future, Grpc::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        match self.web.poll_ready(cx) {
            Poll::Ready(Ok(())) => match self.grpc.poll_ready(cx) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(_e)) => unreachable!(),
                Poll::Pending => Poll::Pending,
            },
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, req: axum::http::Request<WebBody>) -> Self::Future {
        if req.headers().get("content-type").map_or(false, |b| {
            http::HeaderValue::as_bytes(b).starts_with(b"application/grpc")
        }) {
            let (parts, body) = req.into_parts();
            let req = http::Request::from_parts(
                parts,
                axum::body::Body::new(body)
                    .map_err(|e| tonic::Status::from_error(e.into()))
                    .boxed_unsync(),
            );
            HybridFuture::Grpc(self.grpc.call(req))
        } else {
            HybridFuture::Web(self.web.call(req))
        }
    }
}

#[pin_project::pin_project(project = HybridFutureProj)]
pub enum HybridFuture<WebFuture, GrpcFuture> {
    Web(#[pin] WebFuture),
    Grpc(#[pin] GrpcFuture),
}

impl<WebFuture, GrpcFuture, WebError, GrpcError, GrpcBody> Future
    for HybridFuture<WebFuture, GrpcFuture>
where
    WebFuture: Future<Output = Result<axum::response::Response, WebError>>,
    GrpcFuture: Future<Output = Result<http::Response<GrpcBody>, GrpcError>>,
    GrpcBody: http_body::Body<Data = bytes::Bytes, Error = tonic::Status> + Send + 'static,
{
    type Output = Result<axum::response::Response, WebError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context) -> Poll<Self::Output> {
        match self.project() {
            HybridFutureProj::Web(a) => a.poll(cx),
            HybridFutureProj::Grpc(b) => match b.poll(cx) {
                Poll::Ready(Ok(res)) => {
                    let (part, body) = res.into_parts();
                    let resp =
                        axum::response::Response::from_parts(part, axum::body::Body::new(body));
                    Poll::Ready(Ok(resp))
                }
                Poll::Ready(Err(_)) => {
                    unreachable!();
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}
