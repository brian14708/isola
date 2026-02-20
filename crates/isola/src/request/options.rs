use tracing::Span;

use super::{TraceRequest, client::RequestConfig};

type MakeSpan = Box<dyn for<'a> FnMut(&TraceRequest<'a>) -> Span + Send + 'static>;

pub struct RequestOptions {
    pub(crate) make_span: Option<MakeSpan>,
    pub(crate) config: RequestConfig,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestOptions {
    #[must_use]
    pub fn new() -> Self {
        Self {
            make_span: None,
            config: RequestConfig::default(),
        }
    }

    #[must_use]
    pub fn with_make_span<F>(mut self, make_span: F) -> Self
    where
        F: for<'a> FnMut(&TraceRequest<'a>) -> Span + Send + 'static,
    {
        self.make_span = Some(Box::new(make_span));
        self
    }

    #[must_use]
    pub fn with_proxy(mut self, proxy: impl Into<String>) -> Self {
        self.config = self.config.with_proxy(proxy);
        self
    }

    pub(crate) fn make_span(&mut self, request: &TraceRequest<'_>) -> Span {
        self.make_span
            .as_mut()
            .map_or_else(Span::none, |make_span| make_span(request))
    }
}
