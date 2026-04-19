use async_nats::{
    HeaderMap, ToSubject,
    jetstream::context::{PublishAckFuture, PublishError},
    jetstream::message::PublishMessage,
};
use async_trait::async_trait;
use bytes::Bytes;
use serde::Serialize;

use crate::{ServiceContext, ServiceState, header};
use std::{fmt::Debug, sync::Arc};

pub struct StreamContext<Context: ServiceContext> {
    pub service: Arc<ServiceState<Context>>,
    pub(crate) subject_prefix: String,
    pub(crate) jetstream: async_nats::jetstream::Context,
}

impl<Context: ServiceContext> StreamContext<Context> {
    pub fn context(&self) -> &Context {
        &self.service.private
    }

    pub fn nats(&self) -> async_nats::Client {
        self.jetstream.client()
    }

    pub fn jetstream(&self) -> &async_nats::jetstream::Context {
        &self.jetstream
    }

    /// Publish a message to the stream.
    pub async fn send<Subject: ToSubject>(
        &self,
        subject: Subject,
        message: &impl Serialize,
    ) -> Result<PublishAckFuture, PublishError> {
        self.jetstream
            .send_publish(
                format!("{}.{}", self.subject_prefix, subject.to_subject()),
                PublishMessage::build()
                    .payload(Bytes::from(serde_json::to_vec(message).unwrap()))
                    .header(header::MESSAGE_ID, ulid::Ulid::new().to_string()),
            )
            .await
    }

    /// Publish a message with headers to the stream.
    pub async fn send_with_headers<Subject: ToSubject>(
        &self,
        subject: Subject,
        headers: HeaderMap,
        message: &impl Serialize,
    ) -> Result<PublishAckFuture, PublishError> {
        self.jetstream
            .send_publish(
                format!("{}.{}", self.subject_prefix, subject.to_subject()),
                PublishMessage::build()
                    .payload(Bytes::from(serde_json::to_vec(message).unwrap()))
                    .headers(headers)
                    .header(header::MESSAGE_ID, ulid::Ulid::new().to_string()),
            )
            .await
    }
}

#[async_trait]
pub trait StreamHandler<Context>: Debug + Send + Sync
where
    Context: ServiceContext,
{
    async fn handle_stream(
        &self,
        rqctx: StreamContext<Context>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
