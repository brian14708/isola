use std::{io, net::IpAddr};

#[async_trait::async_trait]
pub trait DnsResolver: Send + Sync + 'static {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TokioDnsResolver;

#[async_trait::async_trait]
impl DnsResolver for TokioDnsResolver {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
        let addrs = tokio::net::lookup_host((host, port)).await?;
        let mut out = Vec::new();
        for addr in addrs {
            let ip = addr.ip();
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
        Ok(out)
    }
}
