pub enum TraceRequest<'a> {
    Http(&'a http::request::Parts),
    Grpc(&'a http::request::Parts),
}

#[macro_export]
macro_rules! request_span {
    ($request:ident, $($field:tt)*) => {
        match $request {
            $crate::TraceRequest::Http(request) => {
                ::tracing::span!(
                    $($field)*
                    otel.kind = "client",
                    { ::opentelemetry_semantic_conventions::attribute::HTTP_REQUEST_METHOD } = request.method.as_str(),
                    { ::opentelemetry_semantic_conventions::attribute::SERVER_ADDRESS } = request.uri.host().unwrap_or_default(),
                    { ::opentelemetry_semantic_conventions::attribute::SERVER_PORT } = request.uri.port_u16().unwrap_or_else(|| {
                            match request.uri.scheme_str() {
                                Some("http") => 80,
                                Some("https") => 443,
                                _ => 0,
                            }
                        }).to_string(),
                    { ::opentelemetry_semantic_conventions::attribute::URL_FULL } = request.uri.to_string(),
                    { ::opentelemetry_semantic_conventions::attribute::HTTP_RESPONSE_STATUS_CODE } = Empty,
                    { ::opentelemetry_semantic_conventions::attribute::HTTP_RESPONSE_BODY_SIZE } = Empty,
                    { ::opentelemetry_semantic_conventions::attribute::OTEL_STATUS_CODE } = Empty,
                )
            }
            $crate::TraceRequest::Grpc(request) => {
                let mut seg = request.uri.path_and_query().unwrap().path().splitn(3, '/');
                _ = seg.next();
                let service = seg.next().unwrap_or_default();
                let method = seg.next().unwrap_or_default();
                ::tracing::span!(
                    $($field)*
                    otel.kind = "client",
                    { ::opentelemetry_semantic_conventions::attribute::RPC_SYSTEM } = "grpc",
                    { ::opentelemetry_semantic_conventions::attribute::SERVER_ADDRESS } = request.uri.host().unwrap_or_default(),
                    { ::opentelemetry_semantic_conventions::attribute::SERVER_PORT } = request.uri.port_u16().unwrap_or_else(|| {
                            match request.uri.scheme_str() {
                                Some("http") => 80,
                                Some("https") => 443,
                                _ => 0,
                            }
                        }).to_string(),
                    { ::opentelemetry_semantic_conventions::attribute::RPC_METHOD } = method,
                    { ::opentelemetry_semantic_conventions::attribute::RPC_SERVICE } = service,
                    { ::opentelemetry_semantic_conventions::attribute::RPC_GRPC_STATUS_CODE } = Empty,
                    { ::opentelemetry_semantic_conventions::attribute::OTEL_STATUS_CODE } = Empty,
                )
            }
        }
    }
}
