mod acl;
mod dns;
mod private_ranges;

use http::{Method, Uri};

pub use acl::{AclPolicy, AclPolicyBuilder};
pub use acl::{
    Action as AclAction, HostMatch as AclHostMatch, PortRange as AclPortRange, Rule as AclRule,
    Scheme as AclScheme,
};
pub use dns::{DnsResolver, TokioDnsResolver};
pub(crate) use private_ranges::is_private_ip;

#[derive(Debug, Clone)]
pub struct HttpMeta {
    pub method: Method,
    pub uri: Uri,
}

#[derive(Debug, Clone)]
pub struct WebsocketMeta {
    pub uri: Uri,
}

#[async_trait::async_trait]
pub trait NetworkPolicy: Send + Sync + 'static {
    async fn check_http(&self, meta: &HttpMeta) -> core::result::Result<(), String>;
    async fn check_websocket(&self, meta: &WebsocketMeta) -> core::result::Result<(), String>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AllowAllPolicy;

#[async_trait::async_trait]
impl NetworkPolicy for AllowAllPolicy {
    async fn check_http(&self, _meta: &HttpMeta) -> core::result::Result<(), String> {
        Ok(())
    }

    async fn check_websocket(&self, _meta: &WebsocketMeta) -> core::result::Result<(), String> {
        Ok(())
    }
}
