use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use bytes::{Buf, BufMut, Bytes};
use futures::{Stream, StreamExt};
use http::Uri;
use moka::future::Cache;
use opentelemetry_semantic_conventions::attribute as trace;
use pin_project_lite::pin_project;
use tonic::{
    client::Grpc,
    codec::{BufferSettings, Codec, CompressionEncoding},
    transport::{Channel, ClientTlsConfig, Endpoint},
};
use tracing::{Instrument, Span};

#[derive(Clone)]
pub struct GrpcPool {
    pool: Cache<Uri, Channel>,
}

impl GrpcPool {
    pub fn new() -> Self {
        GrpcPool {
            pool: Cache::builder()
                .max_capacity(1024)
                .time_to_idle(Duration::from_secs(60))
                .build(),
        }
    }

    pub async fn get(&self, uri: Uri) -> Result<Channel, Arc<tonic::transport::Error>> {
        self.pool
            .try_get_with(uri.clone(), async move { Self::connect(uri).await })
            .await
    }

    async fn connect(uri: Uri) -> Result<Channel, tonic::transport::Error> {
        let mut endpoint = Endpoint::from(uri);
        #[cfg(feature = "tls")]
        {
            endpoint = endpoint.tls_config(ClientTlsConfig::default().with_enabled_roots())?
        }
        endpoint.connect().await
    }
}

pub async fn grpc(
    span: Span,
    grpc: GrpcPool,
    request: http::Request<impl Stream<Item = Bytes> + Send + 'static>,
) -> Result<
    http::Response<impl Stream<Item = Result<Bytes, crate::Error>> + Send + 'static>,
    crate::Error,
> {
    let mut uri = request.uri().clone().into_parts();
    uri.path_and_query = Some("".try_into().unwrap());

    let ch = grpc
        .get(Uri::from_parts(uri).unwrap())
        .instrument(span.clone())
        .await?;
    let uri = request.uri().clone();
    let req = tonic::Request::new(request.into_body());
    let mut client = Grpc::new(ch).accept_compressed(CompressionEncoding::Gzip);
    client.ready().instrument(span.clone()).await?;

    let s = span.clone();
    let stream = client
        .streaming(req, uri.path_and_query().unwrap().clone(), RawCodec)
        .await;

    let resp = match stream {
        Ok(s) => {
            span.record(trace::RPC_GRPC_STATUS_CODE, 0);
            s.into_inner()
        }
        Err(e) => {
            span.record(trace::RPC_GRPC_STATUS_CODE, e.code() as u16);
            return Err(e.into());
        }
    };

    let builder = http::response::Builder::new();
    return Ok(builder.body(GrpcInstrumentStream::new(
        s,
        resp.map(|e| match e {
            Ok(e) => Ok(e),
            Err(e) => Err(e.into()),
        }),
    ))?);
}

pin_project! {
    struct GrpcInstrumentStream<S> {
        #[pin]
        stream: S,
        span: tracing::Span,
    }
}

impl<S> GrpcInstrumentStream<S> {
    fn new(span: Span, stream: S) -> Self {
        Self { stream, span }
    }
}

impl<S: Stream<Item = Result<M, E>>, M, E> Stream for GrpcInstrumentStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let span = &this.span;
        let enter = span.enter();
        match this.stream.poll_next(cx) {
            Poll::Ready(None) => {
                span.record(trace::OTEL_STATUS_CODE, "OK");
                drop(enter);
                *this.span = tracing::Span::none();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(f))) => Poll::Ready(Some(Ok(f))),
            Poll::Ready(Some(Err(e))) => {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
                drop(enter);
                *this.span = tracing::Span::none();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

struct RawCodec;

impl Codec for RawCodec {
    type Encode = Bytes;
    type Decode = Bytes;

    type Encoder = RawEncoder;
    type Decoder = RawDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder
    }
}

struct RawEncoder;
struct RawDecoder;

impl tonic::codec::Encoder for RawEncoder {
    type Item = Bytes;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        mut item: Self::Item,
        buf: &mut tonic::codec::EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        buf.put(&mut item);
        Ok(())
    }

    fn buffer_settings(&self) -> BufferSettings {
        BufferSettings::default()
    }
}

impl tonic::codec::Decoder for RawDecoder {
    type Item = Bytes;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        src: &mut tonic::codec::DecodeBuf<'_>,
    ) -> Result<Option<Self::Item>, Self::Error> {
        Ok(Some(src.copy_to_bytes(src.remaining())))
    }
}
