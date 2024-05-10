use std::{borrow::Cow, str::FromStr, sync::Arc};

use anyhow::anyhow;
use http::{HeaderName, HeaderValue};
use opentelemetry_semantic_conventions::trace;
use promptkit_llm::tokenizers::Tokenizer;
use tracing::{field::Empty, span, Instrument};

use promptkit_executor::{Env, EnvError};

use crate::proto::{common::v1::RemoteFile, llm::v1::tokenizer};

#[derive(Clone)]
pub struct VmEnv {
    pub http: reqwest::Client,
    pub cache: moka::future::Cache<String, Arc<dyn Tokenizer + Send + Sync>>,

    pub llm_config: Option<crate::proto::llm::v1::LlmConfig>,
}

impl VmEnv {
    pub fn update<'a>(
        &'a self,
        llm_config: Option<&crate::proto::llm::v1::LlmConfig>,
    ) -> Cow<'a, Self> {
        if let Some(llm_config) = llm_config {
            Cow::Owned(Self {
                http: self.http.clone(),
                cache: self.cache.clone(),
                llm_config: Some(llm_config.clone()),
            })
        } else {
            Cow::Borrowed(self)
        }
    }
}

impl Env for VmEnv {
    fn hash(&self, mut update: impl FnMut(&[u8])) {
        if let Some(llm_config) = &self.llm_config {
            for t in &llm_config.tokenizers {
                update(t.name.as_bytes());
                update(&t.r#type.to_be_bytes());
                match &t.source {
                    Some(tokenizer::Source::RemoteFile(RemoteFile { digest, .. })) => {
                        update(digest.as_bytes());
                    }
                    None => unimplemented!(),
                }
            }
        }
    }

    fn send_request(
        &self,
        mut req: reqwest::Request,
    ) -> impl std::future::Future<Output = reqwest::Result<reqwest::Response>> + Send + 'static
    {
        let span = tracing::span!(
            target: "promptkit::http",
            tracing::Level::INFO,
            "http::request_send",
            promptkit.user = true,
            otel.kind = "client",
            { trace::HTTP_REQUEST_METHOD } = req.method().as_str(),
            { trace::SERVER_ADDRESS } = req.url().host_str().unwrap_or_default(),
            { trace::SERVER_PORT } = req.url().port_or_known_default().unwrap_or_default(),
            { trace::URL_FULL } = req.url().to_string(),
            { trace::HTTP_RESPONSE_STATUS_CODE } = Empty,
            { trace::OTEL_STATUS_CODE } = Empty,
        );
        opentelemetry::global::get_text_map_propagator(|injector| {
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            struct RequestCarrier<'a> {
                request: &'a mut reqwest::Request,
            }
            impl<'a> opentelemetry::propagation::Injector for RequestCarrier<'a> {
                fn set(&mut self, key: &str, value: String) {
                    let header_name = HeaderName::from_str(key).expect("Must be header name");
                    let header_value =
                        HeaderValue::from_str(&value).expect("Must be a header value");
                    self.request.headers_mut().insert(header_name, header_value);
                }
            }

            let context = span.context();
            injector.inject_context(&context, &mut RequestCarrier { request: &mut req });
        });

        let http = self.http.clone();
        async move {
            let resp = match http.execute(req).instrument(span.clone()).await {
                Ok(resp) => resp,
                Err(err) => {
                    span.record(trace::OTEL_STATUS_CODE, "ERROR");
                    return Err(err);
                }
            };

            let status = resp.status();
            span.record("http.response.status_code", status.as_u16());
            if status.is_server_error() || status.is_client_error() {
                span.record(trace::OTEL_STATUS_CODE, "ERROR");
            } else {
                span.record(trace::OTEL_STATUS_CODE, "OK");
            }

            Ok(resp)
        }
    }

    async fn get_tokenizer(
        &self,
        name: &str,
    ) -> Result<Arc<dyn Tokenizer + Send + Sync>, EnvError> {
        if let Some(llm_config) = &self.llm_config {
            for t in &llm_config.tokenizers {
                if t.name == name {
                    match &t.source {
                        Some(tokenizer::Source::RemoteFile(RemoteFile { digest, url })) => {
                            let tokenizer = self.cache.try_get_with::<_, EnvError>(
                                digest.clone(),
                                async move {
                                    let span = span!(
                                        target: "promptkit::llm",
                                        tracing::Level::INFO,
                                        "llm::tokenizer::initialize",
                                        promptkit.user = true,
                                    );
                                    let req = {
                                        let _guard = span.enter();
                                        self.send_request(reqwest::Request::new(
                                            reqwest::Method::GET,
                                            reqwest::Url::parse(url)
                                                .map_err(|e| EnvError::Internal(e.into()))?,
                                        ))
                                    };

                                    let resp = req.instrument(span.clone()).await?;
                                    let bytes = resp.bytes().instrument(span.clone()).await?;
                                    let _guard = span.enter();
                                    let tokenizer = promptkit_llm::tokenizers::load_spm(&bytes)
                                        .map_err(|e| EnvError::Internal(e.into()))?;
                                    Ok(Arc::new(tokenizer) as Arc<dyn Tokenizer + Send + Sync>)
                                },
                            );
                            return tokenizer.await.map_err(|e| {
                                Arc::try_unwrap(e).unwrap_or_else(|_| {
                                    EnvError::Internal(anyhow!("unknown error"))
                                })
                            });
                        }
                        None => unimplemented!(),
                    }
                }
            }
        }
        Err(EnvError::NotFound)
    }
}
