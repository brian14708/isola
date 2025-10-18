use std::str::FromStr;

use bytes::Bytes;
use futures::Stream;
use http::{HeaderName, HeaderValue};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tracing::{Instrument, Span};

use crate::{
    Error, RequestOptions, WebsocketMessage, client::pool::ClientPool, options::RequestContext,
    trace::TraceRequest,
};

mod builder;
mod config;
mod pool;

pub use builder::ClientBuilder;
pub use config::RequestConfig;

pub struct Client {
    pool: ClientPool,
    cleanup_task: JoinHandle<()>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Create a new request client.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Create a new client builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub(crate) fn build(c: &ClientBuilder) -> Self {
        let pool = ClientPool::new(c.max_inflight_per_client);
        let cleanup_task = {
            let cleanup_interval = c.client_idle_timeout;
            let pool = pool.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(cleanup_interval / 2);
                loop {
                    interval.tick().await;
                    pool.cleanup(cleanup_interval);
                }
            })
        };

        Self { pool, cleanup_task }
    }

    /// Send an HTTP request.
    ///
    /// # Errors
    /// Returns error if request fails.
    pub async fn http<B, C>(
        &self,
        request: http::Request<B>,
        options: RequestOptions<C>,
    ) -> Result<
        http::Response<impl Stream<Item = Result<http_body::Frame<Bytes>, Error>> + 'static>,
        Error,
    >
    where
        B: http_body::Body + 'static,
        B::Error: std::error::Error + Send + Sync,
        B::Data: Send,
        C: RequestContext,
    {
        self.with_http_client(request, options, |client, request| {
            crate::http::http_impl(client, request)
        })
        .await
    }

    /// Send a WebSocket request.
    ///
    /// # Errors
    /// Returns error if WebSocket connection fails.
    pub async fn websocket<C: RequestContext, B>(
        &self,
        request: http::Request<B>,
        options: RequestOptions<C>,
    ) -> Result<http::Response<impl Stream<Item = Result<WebsocketMessage, Error>> + 'static>, Error>
    where
        B: Stream<Item = WebsocketMessage> + Send + 'static,
    {
        self.with_http_client(request, options, |client, mut request: http::Request<B>| {
            let headers = request.headers_mut();
            headers.insert(
                HeaderName::from_static("connection"),
                HeaderValue::from_static("Upgrade"),
            );
            headers.insert(
                HeaderName::from_static("upgrade"),
                HeaderValue::from_static("websocket"),
            );
            headers.insert(
                HeaderName::from_static("sec-websocket-version"),
                HeaderValue::from_static("13"),
            );
            headers.insert(
                HeaderName::from_static("sec-websocket-key"),
                #[expect(
                    clippy::missing_panics_doc,
                    reason = "generate_key only returns base64"
                )]
                HeaderValue::try_from(generate_key()).unwrap(),
            );

            crate::http::websocket_impl(client, request)
        })
        .await
    }

    async fn with_http_client<B, C: RequestContext, T>(
        &self,
        mut request: http::Request<B>,
        mut options: RequestOptions<C>,
        f: impl AsyncFnOnce(reqwest::Client, http::Request<B>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut config = options.config;

        if let Some(p) = request.headers_mut().remove("x-promptkit-proxy") {
            let proxy_str = p
                .to_str()
                .map_err(|_e| Error::Url(url::ParseError::EmptyHost))?;
            config = config.with_proxy(proxy_str.to_string());
        }

        let (parts, body) = request.into_parts();
        let span = options.context.make_span(&TraceRequest::Http(&parts));
        let request = inject_headers(&span, http::Request::from_parts(parts, body));

        let token = self.pool.reserve(config)?;
        f(token.client.clone(), request).instrument(span).await
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.cleanup_task.abort();
    }
}

fn inject_headers<B>(span: &Span, mut request: http::Request<B>) -> http::Request<B> {
    opentelemetry::global::get_text_map_propagator(|injector| {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        struct RequestCarrier<'a, T>(&'a mut http::Request<T>);
        impl<T> opentelemetry::propagation::Injector for RequestCarrier<'_, T> {
            fn set(&mut self, key: &str, value: String) {
                if let (Ok(header_name), Ok(header_value)) =
                    (HeaderName::from_str(key), HeaderValue::try_from(value))
                {
                    self.0.headers_mut().insert(header_name, header_value);
                }
            }
        }

        let context = span.context();
        injector.inject_context(&context, &mut RequestCarrier(&mut request));
    });
    request
}
