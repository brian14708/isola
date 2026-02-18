use tracing::Span;

use crate::{TraceRequest, client::RequestConfig};

pub struct RequestOptions<C = ()>
where
    C: RequestContext,
{
    pub(crate) context: C,
    pub(crate) config: RequestConfig,
}

impl Default for RequestOptions<()> {
    fn default() -> Self {
        Self::new(())
    }
}

impl<C: RequestContext> RequestOptions<C> {
    #[must_use]
    pub fn new(c: C) -> Self {
        Self {
            context: c,
            config: RequestConfig::default(),
        }
    }

    #[must_use]
    pub fn with_proxy(mut self, proxy: String) -> Self {
        self.config = self.config.with_proxy(proxy);
        self
    }
}

pub trait RequestContext {
    fn make_span(&mut self, _: &TraceRequest) -> Span;
}

impl RequestContext for () {
    fn make_span(&mut self, _: &TraceRequest) -> Span {
        Span::none()
    }
}
