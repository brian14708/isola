use futures_util::Stream;
use tonic::Status;

use crate::proto::script::{
    script_service_server::ScriptService, ExecuteClientStreamRequest, ExecuteClientStreamResponse,
    ExecuteRequest, ExecuteResponse, ExecuteServerStreamRequest, ExecuteServerStreamResponse,
    ExecuteStreamRequest, ExecuteStreamResponse,
};

pub struct ScriptServer;

pub struct ExecuteServerStreamStream;
pub struct ExecuteStreamStream;

#[tonic::async_trait]
impl ScriptService for ScriptServer {
    type ExecuteServerStreamStream = ExecuteServerStreamStream;
    type ExecuteStreamStream = ExecuteStreamStream;

    async fn execute(
        &self,
        _request: tonic::Request<ExecuteRequest>,
    ) -> Result<tonic::Response<ExecuteResponse>, Status> {
        todo!()
    }

    async fn execute_client_stream(
        &self,
        _request: tonic::Request<tonic::Streaming<ExecuteClientStreamRequest>>,
    ) -> Result<tonic::Response<ExecuteClientStreamResponse>, Status> {
        todo!()
    }

    async fn execute_server_stream(
        &self,
        _request: tonic::Request<ExecuteServerStreamRequest>,
    ) -> Result<tonic::Response<Self::ExecuteServerStreamStream>, Status> {
        todo!()
    }

    async fn execute_stream(
        &self,
        _request: tonic::Request<tonic::Streaming<ExecuteStreamRequest>>,
    ) -> Result<tonic::Response<Self::ExecuteStreamStream>, Status> {
        todo!()
    }
}

impl Stream for ExecuteServerStreamStream {
    type Item = Result<ExecuteServerStreamResponse, Status>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context,
    ) -> std::task::Poll<Option<Self::Item>> {
        todo!()
    }
}

impl Stream for ExecuteStreamStream {
    type Item = Result<ExecuteStreamResponse, Status>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context,
    ) -> std::task::Poll<Option<Self::Item>> {
        todo!()
    }
}
