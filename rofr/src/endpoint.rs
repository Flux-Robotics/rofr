use crate::ServiceContext;
use crate::ServiceState;
use async_trait::async_trait;
use bytes::Bytes;
use serde::Deserialize;
use serde::Serialize;
use std::fmt::Debug;
use std::sync::Arc;

/// Request wrapper type for endpoint request bodies
#[derive(Debug, Serialize, Deserialize)]
pub struct Request<T> {
    #[serde(flatten)]
    pub inner: T,
}

impl<T: serde::de::DeserializeOwned + Serialize> Request<T> {
    /// Serialize the request into raw bytes.
    ///
    /// This is the counterpart to [`Response::from_bytes`] and can be used to
    /// manually implement [`EndpointHandler`] without the [`service`] macro.
    pub fn into_bytes(self) -> Result<Bytes, serde_json::Error> {
        serde_json::to_vec(&self.inner).map(Bytes::from)
    }

    /// Deserialize a [`Request`] from raw bytes.
    ///
    /// This is the counterpart to [`Response::into_bytes`] and can be used to
    /// manually implement [`EndpointHandler`] without the [`service`] macro.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

impl<T> Request<T> {
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> std::ops::Deref for Request<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Successful response wrapper
#[derive(Debug, Serialize, Deserialize)]
pub struct Response<T>(pub T);

impl<T: serde::de::DeserializeOwned + Serialize> Response<T> {
    /// Serialize the response into raw bytes.
    ///
    /// This is the counterpart to [`Request::from_bytes`] and can be used to
    /// manually implement [`EndpointHandler`] without the [`service`] macro.
    pub fn into_bytes(self) -> Result<Bytes, serde_json::Error> {
        serde_json::to_vec(&self).map(Bytes::from)
    }

    /// Deserialize a [`Response`] from raw bytes.
    ///
    /// This is the counterpart to [`Request::into_bytes`] and can be used to
    /// manually implement [`EndpointHandler`] without the [`service`] macro.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Request context.
#[non_exhaustive]
pub struct RequestContext<Context: ServiceContext> {
    pub(crate) nats: async_nats::Client,
    /// Service shared state.
    pub service: Arc<ServiceState<Context>>,
    /// Unique id for this request. Relies on the client generating this.
    pub request_id: String,
}

impl<Context: ServiceContext> RequestContext<Context> {
    /// Shared context.
    pub fn context(&self) -> &Context {
        &self.service.private
    }

    /// NATS connection.
    pub fn nats(&self) -> &async_nats::Client {
        &self.nats
    }
}

#[async_trait]
pub trait EndpointHandler<Context>: Debug + Send + Sync
where
    Context: ServiceContext,
{
    async fn handle_request(
        &self,
        rqctx: RequestContext<Context>,
        body: Bytes,
    ) -> Result<Bytes, Box<dyn std::error::Error + Send + Sync>>;
}
