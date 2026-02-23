use std::{str::FromStr, time::Duration};

use bytes::Bytes;
use futures::Stream;
use http::{HeaderName, HeaderValue};
use tokio::sync::oneshot;
use tracing::{Instrument, Span, warn};

use self::pool::ClientPool;
use super::{Error, RequestOptions, trace::TraceRequest};

mod builder;
mod config;
mod pool;

pub use builder::ClientBuilder;
pub use config::RequestConfig;

pub struct Client {
    pool: ClientPool,
    cleanup_stop: Option<oneshot::Sender<()>>,
    cleanup_task: Option<tokio::task::JoinHandle<()>>,
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
        let cleanup_interval = c.client_idle_timeout.max(Duration::from_millis(1));
        let cleanup_period = cleanup_interval / 2;

        let (cleanup_stop, cleanup_task) = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let pool = pool.clone();
                let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
                let task = handle.spawn(async move {
                    let mut interval = tokio::time::interval(cleanup_period);
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                    loop {
                        tokio::select! {
                            _ = &mut stop_rx => break,
                            _ = interval.tick() => pool.cleanup(cleanup_interval),
                        }
                    }
                });
                (Some(stop_tx), Some(task))
            }
            Err(e) => {
                warn!(
                    "failed to spawn request client cleanup task: no tokio runtime available ({e})"
                );
                (None, None)
            }
        };

        Self {
            pool,
            cleanup_stop,
            cleanup_task,
        }
    }

    /// Send an HTTP request.
    ///
    /// # Errors
    /// Returns error if request fails.
    pub async fn send_http(
        &self,
        request: http::Request<Bytes>,
        options: RequestOptions,
    ) -> Result<
        http::Response<impl Stream<Item = Result<http_body::Frame<Bytes>, Error>> + 'static>,
        Error,
    > {
        self.with_http_client(request, options, |client, request| {
            super::http::http_impl(client, request)
        })
        .await
    }

    async fn with_http_client<B, T>(
        &self,
        mut request: http::Request<B>,
        mut options: RequestOptions,
        f: impl AsyncFnOnce(reqwest::Client, http::Request<B>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let config = options.config.clone();
        // Proxy configuration is explicit via `RequestOptions`; this header is ignored.
        request.headers_mut().remove("x-isola-proxy");

        let (parts, body) = request.into_parts();
        let span = options.make_span(&TraceRequest::Http(&parts));
        let request = inject_headers(&span, http::Request::from_parts(parts, body));

        let token = self.pool.reserve(config)?;
        f(token.client.clone(), request).instrument(span).await
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(stop) = self.cleanup_stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.cleanup_task.take() {
            task.abort();
        }
    }
}

fn inject_headers<B>(span: &Span, mut request: http::Request<B>) -> http::Request<B> {
    opentelemetry::global::get_text_map_propagator(|injector| {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        struct RequestCarrier<'a, T>(&'a mut http::Request<T>);
        impl<T> opentelemetry::propagation::Injector for RequestCarrier<'_, T> {
            fn set(&mut self, key: &str, value: String) {
                if value.is_empty() {
                    return;
                }
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
