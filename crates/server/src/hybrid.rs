use std::{future::Future, pin::Pin, str::FromStr, task::Poll};

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
    Grpc: tower::Service<
        http_02::Request<tonic::transport::Body>,
        Response = http_02::Response<GrpcBody>,
    >,
    WebBody: axum::body::HttpBody<Data = bytes::Bytes> + Send + 'static,
    WebBody::Error: Into<axum::BoxError>,
    GrpcBody: http_body_04::Body<Data = bytes::Bytes, Error = tonic::Status> + Send + 'static,
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
            let mut req = http_02::Request::new(tonic::transport::Body::wrap_stream(
                axum::body::Body::new(body).into_data_stream(),
            ));
            *req.version_mut() = match parts.version {
                axum::http::Version::HTTP_10 => http_02::Version::HTTP_10,
                axum::http::Version::HTTP_11 => http_02::Version::HTTP_11,
                axum::http::Version::HTTP_2 => http_02::Version::HTTP_2,
                axum::http::Version::HTTP_3 => http_02::Version::HTTP_3,
                _ => http_02::Version::HTTP_09,
            };
            *req.method_mut() =
                http_02::Method::from_bytes(parts.method.as_str().as_bytes()).unwrap();
            *req.uri_mut() = http_02::Uri::from_str(parts.uri.to_string().as_str()).unwrap();
            req.headers_mut()
                .extend(parts.headers.into_iter().map(|(k, v)| {
                    (
                        k.map(|k| http_02::HeaderName::from_bytes(k.as_ref()).unwrap()),
                        v.to_str().unwrap().parse().unwrap(),
                    )
                }));
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
    GrpcFuture: Future<Output = Result<http_02::Response<GrpcBody>, GrpcError>>,
    GrpcBody: http_body_04::Body<Data = bytes::Bytes, Error = tonic::Status> + Send + 'static,
{
    type Output = Result<axum::response::Response, WebError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context) -> Poll<Self::Output> {
        match self.project() {
            HybridFutureProj::Web(a) => a.poll(cx),
            HybridFutureProj::Grpc(b) => match b.poll(cx) {
                Poll::Ready(Ok(res)) => {
                    let (part, body) = res.into_parts();
                    let mut resp =
                        axum::response::Response::new(axum::body::Body::new(GrpcBodyAdapter {
                            body,
                            body_finished: false,
                        }));
                    *resp.status_mut() = http::StatusCode::from_u16(part.status.as_u16()).unwrap();
                    *resp.version_mut() = match part.version {
                        http_02::Version::HTTP_10 => axum::http::Version::HTTP_10,
                        http_02::Version::HTTP_11 => axum::http::Version::HTTP_11,
                        http_02::Version::HTTP_2 => axum::http::Version::HTTP_2,
                        http_02::Version::HTTP_3 => axum::http::Version::HTTP_3,
                        _ => axum::http::Version::HTTP_09,
                    };
                    resp.headers_mut()
                        .extend(part.headers.into_iter().map(|(k, v)| {
                            (
                                k.map(|k| http::HeaderName::from_bytes(k.as_ref()).unwrap()),
                                v.to_str().unwrap().parse().unwrap(),
                            )
                        }));
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

#[pin_project::pin_project]
struct GrpcBodyAdapter<Body> {
    #[pin]
    body: Body,
    body_finished: bool,
}

impl<Body> axum::body::HttpBody for GrpcBodyAdapter<Body>
where
    Body: http_body_04::Body<Data = bytes::Bytes, Error = tonic::Status>,
{
    type Data = bytes::Bytes;
    type Error = axum::BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        if !*this.body_finished {
            match this.body.as_mut().poll_data(cx) {
                Poll::Ready(Some(Ok(data))) => {
                    return Poll::Ready(Some(Ok(http_body::Frame::data(data))))
                }
                Poll::Ready(None) => {
                    *this.body_finished = true;
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e.into()))),
                Poll::Pending => return Poll::Pending,
            }
        }
        match this.body.as_mut().poll_trailers(cx) {
            Poll::Ready(Ok(Some(trailers))) => {
                let mut copy = http::HeaderMap::new();
                for (k, v) in &trailers {
                    copy.append(
                        http::HeaderName::from_str(k.as_str()).unwrap(),
                        v.to_str().unwrap().parse().unwrap(),
                    );
                }
                Poll::Ready(Some(Ok(http_body::Frame::trailers(copy))))
            }
            Poll::Ready(Ok(None)) => Poll::Ready(None),
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e.into()))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        let mut s = http_body::SizeHint::default();
        let b = self.body.size_hint();
        s.set_lower(b.lower());
        if let Some(u) = b.upper() {
            s.set_upper(u);
        }
        s
    }
}
