use super::super::Error;

const USER_AGENT: &str = "Isola/1.0";

#[derive(Default, Clone, PartialEq, Eq, Hash)]
pub struct RequestConfig {
    pub proxy: Option<String>,
}

impl RequestConfig {
    #[must_use]
    pub fn with_proxy(mut self, proxy: impl Into<String>) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    pub fn build_client(&self) -> Result<reqwest::Client, Error> {
        let mut builder = reqwest::Client::builder().user_agent(USER_AGENT);
        if let Some(proxy_str) = &self.proxy {
            let proxy = reqwest::Proxy::all(proxy_str)?;
            builder = builder.proxy(proxy);
        }
        Ok(builder.build()?)
    }
}
