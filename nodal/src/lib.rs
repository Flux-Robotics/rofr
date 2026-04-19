#![doc = include_str!("../../README.md")]

pub mod endpoint;
pub mod header;
pub mod stream;

pub use async_trait;
pub use bytes::Bytes;
pub use endpoint::{BoxError, EndpointHandler};
pub use nodal_macros::{endpoint, service, stream};

use async_nats::ConnectOptions;
use async_nats::HeaderMap;
use async_nats::PublishError;
use async_nats::ToServerAddrs;
use async_nats::service::ServiceExt;
use futures::StreamExt;
use header::*;
use schemars::Schema;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use stream::*;
use tokio::task::JoinSet;
use tracing::Level;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::span;

/// Marker trait for service context types
pub trait ServiceContext: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> ServiceContext for T {}

/// Error type for service endpoints
#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

impl From<async_nats::jetstream::context::PublishError> for Error {
    fn from(value: async_nats::jetstream::context::PublishError) -> Self {
        Self {
            message: value.to_string(),
        }
    }
}

pub struct ServiceState<Context: ServiceContext> {
    /// Service-specific state
    private: Context,
    /// Serivce unique id.
    pub uid: String,
}

/// Endpoint definition.
pub struct Endpoint<Context: ServiceContext> {
    pub subject: String,
    pub handler: Arc<dyn EndpointHandler<Context>>,
    pub request_schema: Schema,
    pub response_schema: Schema,
}

pub struct Stream<Context: ServiceContext> {
    pub subject_prefix: String,
    pub config: async_nats::jetstream::stream::Config,
    pub handler: Arc<dyn StreamHandler<Context>>,
    pub message_schema: Schema,
}

/// Service definition.
pub struct Service<Context: ServiceContext> {
    pub name: String,
    pub version: String,
    pub endpoints: Vec<Endpoint<Context>>,
    pub streams: Vec<Stream<Context>>,
    pub context: Context,
}

/// Cluster definition.
pub struct Cluster<Context: ServiceContext, A: ToServerAddrs> {
    nats_addrs: A,
    nats_options: ConnectOptions,
    services: Vec<Service<Context>>,
}

impl<Context: ServiceContext, A: ToServerAddrs> Cluster<Context, A> {
    pub fn new(addrs: A) -> std::io::Result<Self> {
        Ok(Self {
            nats_addrs: addrs,
            nats_options: ConnectOptions::default(),
            services: Vec::new(),
        })
    }

    pub fn new_with_options(addrs: A, options: ConnectOptions) -> std::io::Result<Self> {
        Ok(Self {
            nats_addrs: addrs,
            nats_options: options,
            services: Vec::new(),
        })
    }

    /// Register service instance.
    pub fn register(&mut self, d: Service<Context>) {
        self.services.push(d);
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let new_client = async || {
            Ok::<_, Box<dyn std::error::Error>>(
                async_nats::connect_with_options(&self.nats_addrs, self.nats_options.clone())
                    .await
                    .map_err(|e| format!("NATS connect failed: {e}"))?,
            )
        };

        let mut join_set = tokio::task::JoinSet::new();

        for service in self.services {
            let nats = new_client().await?;
            let nats_service = nats
                .service_builder()
                .metadata(HashMap::from([(
                    VERSION.to_owned(),
                    env!("CARGO_PKG_VERSION").to_owned(),
                )]))
                .start(&service.name, &service.version)
                .await
                .map_err(|e| e.to_string())?;
            let span = span!(
                Level::INFO,
                "service",
                name = service.name,
                version = service.version,
            );

            join_set.spawn(async move {
                let _guard = span.enter();
                if let Err(e) = run_service(nats, service, nats_service).await {
                    eprintln!("Service task: panicked: {e}");
                }
                drop(_guard);
            });
        }

        while let Some(res) = join_set.join_next().await {
            if let Err(e) = res {
                eprintln!("Service task panicked: {e}");
            }
        }

        Ok(())
    }
}

async fn run_service<Context: ServiceContext>(
    nats: async_nats::Client,
    service: Service<Context>,
    nats_service: async_nats::service::Service,
) -> Result<(), async_nats::Error> {
    let service_state = Arc::new(ServiceState {
        private: service.context,
        uid: nats_service.info().await.id.clone(),
    });

    let mut join_set: JoinSet<Result<_, PublishError>> = JoinSet::new();

    for endpoint in service.endpoints.iter() {
        let span = span!(Level::INFO, "endpoint", "subject" = endpoint.subject);
        let nats = nats.clone();
        let service_state = service_state.clone();
        let handler = endpoint.handler.clone();
        let subject = format!("{}.{}", service.name, endpoint.subject);

        let mut ep = nats_service
            .endpoint_builder()
            .metadata(HashMap::from([
                (
                    REQUEST_SCHEMA.to_owned(),
                    serde_json::to_string(&endpoint.request_schema)?,
                ),
                (
                    RESPONSE_SCHEMA.to_owned(),
                    serde_json::to_string(&endpoint.response_schema)?,
                ),
            ]))
            .add(subject)
            .await?;

        join_set.spawn(async move {
            let _guard = span.enter();
            while let Some(req) = ep.next().await {
                let request_id = req
                    .message
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(header::REQUEST_ID).map(|v| v.as_str()));

                let span = span!(Level::INFO, "handler", request_id = request_id.to_owned());
                let _guard = span.enter();
                let result = handler
                    .handle_request(
                        endpoint::RequestContext {
                            service: service_state.clone(),
                            nats: nats.clone(),
                            request_id: request_id.unwrap_or("").to_owned(),
                        },
                        req.message.payload.clone(),
                    )
                    .await;

                // response headers
                let mut headers = HeaderMap::new();
                headers.insert(header::SERVICE_UID, service_state.uid.as_str());
                if let Some(id) = request_id {
                    headers.insert(header::REQUEST_ID, id);
                }

                let response = match result {
                    Ok(res) => {
                        debug!(response_size_bytes = res.len(), "request completed");
                        Ok(res)
                    }
                    Err(err) => {
                        let message = format!("{}", err);
                        error!(message, "request failed");
                        Err(async_nats::service::error::Error {
                            status: message,
                            code: 0, // todo: not sure what to do with this
                        })
                    }
                };

                req.respond_with_headers(response, headers).await?;
            }
            Ok(())
        });
    }

    for stream in service.streams {
        let nats = nats.clone();
        let jetstream = async_nats::jetstream::new(nats);
        let service = service_state.clone();
        let handler = stream.handler.clone();

        let _ = jetstream.create_or_update_stream(stream.config).await?;

        join_set.spawn(async move {
            handler
                .handle_stream(StreamContext {
                    service,
                    subject_prefix: stream.subject_prefix,
                    jetstream,
                })
                .await
                .unwrap();
            Ok(())
        });
    }

    info!("service started");

    while let Some(res) = join_set.join_next().await {
        if let Err(e) = res {
            eprintln!("Service task panicked: {e}");
        }
    }

    info!("stopping service");
    nats_service.stop().await?;

    Ok(())
}
