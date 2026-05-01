//! RoFr is a general-purpose framework for creating RPC-like APIs in Rust
//! using NATS messaging. It also happens to be good for building robot
//! software.
//!
//! # API stability
//! While this crate is pre v1, the API can and will change without warning.

mod client;
mod endpoint;
pub mod header;
mod stream;

pub use async_trait;
pub use bytes::Bytes;
pub use client::ClientError;
pub use client::generate_request_id;
pub use endpoint::EndpointHandler;
pub use endpoint::Request;
pub use endpoint::RequestContext;
pub use endpoint::Response;
pub use stream::StreamContext;
pub use stream::StreamHandler;

use async_nats::ConnectOptions;
use async_nats::HeaderMap;
use async_nats::ToServerAddrs;
use async_nats::service::ServiceExt;
use futures::StreamExt;
use header::*;
use schemars::Schema;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::Instrument;
use tracing::Level;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::span;

extern crate rofr_macros;

/// Generates a service API description from a trait.
///
/// The `Context` type is shared between endpoints and stream handlers to act as
/// the internal state of the service. The type is assigned only in the
/// implementation, but traits can be applied here like: `type Context: Debug;`
///
/// Within the trait are the [`endpoint`] and [`stream`] definitions.
///
/// # Headers
///
/// - `RoFr-Version` RoFr library version.
///
/// # Example
///
/// ```rust
/// use rofr::Error;
/// use rofr::Request;
/// use rofr::RequestContext;
/// use rofr::Response;
/// use rofr::service;
///
/// #[service(name = "actuator", version = "0.1.2")]
/// trait ActuatorService {
///     type Context;
///
///     #[endpoint(subject = "set_torque")]
///     async fn set_torque(
///         ctx: RequestContext<Self::Context>,
///         body: Request<f64>,
///     ) -> Result<Response<()>, Error>;
/// }
/// ```
pub use rofr_macros::service;

/// Transforms a handler function into a NATS service endpoint.
///
/// # Request Headers
///
/// - `RoFr-Request-Id` (optional) a unique identifier that the client generates to help trace requests.
///
/// ```ignore
/// #[endpoint(subject = "example")]
/// async fn example_endpoint(
///     ctx: RequestContext<Self::Context>,
///     /* ... */,
/// ) -> Result<Response</* ... */, Error>;
/// ```
pub use rofr_macros::endpoint;

/// Transforms a stream handler function into a NATS JetStream publisher.
///
/// ```ignore
/// #[stream(
///     name = "EXAMPLE",
///     subject = "example",
///     message = ExampleType,
/// )]
/// async fn example(ctx: StreamContext<Self::Context>) -> Result<(), Error>;
/// ```
pub use rofr_macros::stream;

/// Marker trait for service context types.
pub trait ServiceContext: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> ServiceContext for T {}

/// Error type for service endpoints.
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

/// Shared service state.
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

/// Stream definition.
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

/// Type-erased service that can be run by the cluster, regardless of its context type.
trait AnyService: Send + 'static {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn run(
        self: Box<Self>,
        nats: async_nats::Client,
        nats_service: async_nats::service::Service,
    ) -> Pin<Box<dyn Future<Output = Result<(), async_nats::Error>> + Send + 'static>>;
}

impl<Context: ServiceContext> AnyService for Service<Context> {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn run(
        self: Box<Self>,
        nats: async_nats::Client,
        nats_service: async_nats::service::Service,
    ) -> Pin<Box<dyn Future<Output = Result<(), async_nats::Error>> + Send + 'static>> {
        Box::pin(run_service(nats, *self, nats_service))
    }
}

/// Cluster definition.
pub struct Cluster<A: ToServerAddrs> {
    nats_addrs: A,
    nats_options: ConnectOptions,
    services: Vec<Box<dyn AnyService>>,
}

impl<A: ToServerAddrs> Cluster<A> {
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
    pub fn register<Context: ServiceContext>(&mut self, d: Service<Context>) {
        self.services.push(Box::new(d));
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let new_client = async || {
            Ok::<_, Box<dyn std::error::Error>>(
                async_nats::connect_with_options(&self.nats_addrs, self.nats_options.clone())
                    .await
                    .map_err(|e| format!("NATS connect failed: {e}"))?,
            )
        };

        let mut join_set = tokio::task::JoinSet::<Result<_, async_nats::Error>>::new();

        for service in self.services {
            let nats = new_client().await?;
            let name = service.name().to_owned();
            let version = service.version().to_owned();
            let nats_service = nats
                .service_builder()
                .metadata(HashMap::from([(
                    VERSION.to_owned(),
                    env!("CARGO_PKG_VERSION").to_owned(),
                )]))
                .start(&name, &version)
                .await
                .map_err(|e| e.to_string())?;
            let span = span!(Level::INFO, "service", name = name, version = version);

            join_set.spawn(
                async move {
                    service.run(nats, nats_service).await?;
                    Ok(())
                }
                .instrument(span),
            );
        }

        while let Some(res) = join_set.join_next().await {
            res.map_err(|e| format!("join error: {e}"))?
                .map_err(|e| format!("service task stopped: {e}"))?;
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

    let mut join_set: JoinSet<Result<_, async_nats::Error>> = JoinSet::new();

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

        #[cfg(feature = "metrics")]
        let metric_labels = [
            ("service_name", service.name.to_owned()),
            ("service_version", service.version.to_owned()),
            (
                "endpoint_subject",
                format!("{}.{}", service.name, endpoint.subject),
            ),
        ];

        join_set.spawn(
            async move {
                debug!("handler started");
                while let Some(req) = ep.next().await {
                    #[cfg(feature = "metrics")]
                    let start = std::time::Instant::now();

                    let request_id = req
                        .message
                        .headers
                        .as_ref()
                        .and_then(|h| h.get(header::REQUEST_ID).map(|v| v.as_str()));

                    let span = span!(Level::INFO, "handler", request_id = request_id.to_owned());
                    #[cfg(feature = "metrics")]
                    let start_handler = std::time::Instant::now();
                    let result = handler
                        .handle_request(
                            endpoint::RequestContext {
                                service: service_state.clone(),
                                nats: nats.clone(),
                                request_id: request_id.unwrap_or("").to_owned(),
                            },
                            req.message.payload.clone(),
                        )
                        .instrument(span)
                        .await;
                    #[cfg(feature = "metrics")]
                    metrics::histogram!("rofr_request_handler_duration_seconds", &metric_labels)
                        .record(start_handler.elapsed().as_secs_f64());

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
                            #[cfg(feature = "metrics")]
                            metrics::counter!("rofr_request_handler_errors", &metric_labels)
                                .increment(1);
                            Err(async_nats::service::error::Error {
                                status: message,
                                code: 0, // todo: not sure what to do with this
                            })
                        }
                    };

                    req.respond_with_headers(response, headers).await?;

                    #[cfg(feature = "metrics")]
                    metrics::histogram!("rofr_request_duration_seconds", &metric_labels)
                        .record(start.elapsed().as_secs_f64());
                }
                info!("handler ended");
                Ok(())
            }
            .instrument(span),
        );
    }

    for stream in service.streams {
        let span = span!(
            Level::INFO,
            "stream",
            "subject_prefix" = stream.subject_prefix
        );
        let nats = nats.clone();
        let jetstream = async_nats::jetstream::new(nats);
        let service = service_state.clone();
        let handler = stream.handler.clone();

        let _ = jetstream.create_or_update_stream(stream.config).await?;

        join_set.spawn(
            async move {
                debug!("handler started");
                handler
                    .handle_stream(StreamContext {
                        service,
                        subject_prefix: stream.subject_prefix,
                        jetstream,
                    })
                    .await?;
                info!("handler ended");
                Ok(())
            }
            .instrument(span),
        );
    }

    while let Some(res) = join_set.join_next().await {
        res.map_err(|e| format!("service task stopped: {e}"))??;
    }

    info!("stopping service");
    nats_service.stop().await?;

    Ok(())
}
