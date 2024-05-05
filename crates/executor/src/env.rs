#[async_trait::async_trait]
pub trait Env {
    async fn send_request(&self, request: reqwest::Request) -> reqwest::Result<reqwest::Response>;
}
