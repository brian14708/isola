use super::Client;

pub const DEFAULT_MAX_INFLIGHT_PER_CLIENT: u32 = 96;
pub const DEFAULT_CLIENT_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

#[must_use]
pub struct ClientBuilder {
    pub(crate) max_inflight_per_client: u32,
    pub(crate) client_idle_timeout: std::time::Duration,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            max_inflight_per_client: DEFAULT_MAX_INFLIGHT_PER_CLIENT,
            client_idle_timeout: DEFAULT_CLIENT_IDLE_TIMEOUT,
        }
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn max_inflight_per_client(mut self, max_inflight_per_client: u32) -> Self {
        self.max_inflight_per_client = max_inflight_per_client;
        self
    }

    pub const fn client_idle_timeout(mut self, client_idle_timeout: std::time::Duration) -> Self {
        self.client_idle_timeout = client_idle_timeout;
        self
    }

    #[must_use]
    pub fn build(self) -> Client {
        Client::build(&self)
    }
}
