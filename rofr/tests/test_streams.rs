use async_nats::HeaderMap;
use futures::StreamExt;
use rofr::ClientError;
use rofr::Cluster;
use rofr::Error;
use rofr::StreamContext;
use rofr::service;
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use tokio::time::sleep;

#[service(name = "test_service_with_stream", version = "0.1.2")]
trait TestServiceWithStream {
    type Context;

    #[stream(
        name = "TEST_STREAM",
        subject = "test_stream_subject",
        message = u64,
    )]
    async fn test_stream(ctx: StreamContext<Self::Context>) -> Result<(), Error>;

    #[stream(
        name = "TEST_STREAM_HEADERS",
        subject = "test_stream_headers_subject",
        message = u64,
    )]
    async fn test_stream_headers(ctx: StreamContext<Self::Context>) -> Result<(), Error>;
}

#[derive(Debug)]
struct TestServiceWithStreamImpl;

impl TestServiceWithStream for TestServiceWithStreamImpl {
    type Context = ();

    async fn test_stream(ctx: StreamContext<Self::Context>) -> Result<(), Error> {
        ctx.send("test_stream_subject", &25)
            .await? // publish to NATS
            .await?; // wait for ack from NATS

        // deliberately only publish one message
        Ok(())
    }

    async fn test_stream_headers(ctx: StreamContext<Self::Context>) -> Result<(), Error> {
        let mut headers = HeaderMap::new();
        headers.append("Test-Header", "example");

        ctx.send_with_headers("test_stream_headers_subject", headers, &25)
            .await? // publish to NATS
            .await?; // wait for ack from NATS

        // deliberately only publish one message
        Ok(())
    }
}

#[tokio::test]
async fn test_service_stream() {
    let server = nats_server::run_server("tests/nats/default.conf");

    let mut cluster = Cluster::new(server.client_url()).unwrap();
    let test_service = TestServiceWithStreamImpl::service(());
    cluster.register(test_service);

    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });

    // give the service time to start and publish some messages.
    sleep(Duration::from_millis(50)).await;

    let client = async_nats::connect(server.client_url()).await.unwrap();
    let client = TestServiceWithStreamClient::new(client);

    let mut stream = client.test_stream().await.unwrap();
    let response = stream.next().await;
    assert!(response.is_some());
    assert_eq!(response.unwrap().unwrap(), 25);
}

/// Returns an error when the NATS stream has never been registered by a service.
#[tokio::test]
async fn test_service_stream_not_found() {
    let server = nats_server::run_server("tests/nats/default.conf");
    let client =
        TestServiceWithStreamClient::new(async_nats::connect(server.client_url()).await.unwrap());

    let result = client.test_stream().await;
    assert!(
        matches!(result, Err(ClientError::Request(_))),
        "expected a request error when the stream does not exist",
    );
}

/// A service impl that publishes three sequential values to the same stream.
#[derive(Debug)]
struct TestServiceWithStreamMultipleImpl;

impl TestServiceWithStream for TestServiceWithStreamMultipleImpl {
    type Context = ();

    async fn test_stream(ctx: StreamContext<Self::Context>) -> Result<(), Error> {
        for value in [10u64, 20, 30] {
            ctx.send("test_stream_subject", &value).await?.await?;
        }
        Ok(())
    }

    async fn test_stream_headers(ctx: StreamContext<Self::Context>) -> Result<(), Error> {
        let mut headers = HeaderMap::new();
        headers.append("Test-Header", "example");

        for value in [10u64, 20, 30] {
            ctx.send_with_headers("test_stream_header_subject", headers.clone(), &value)
                .await?
                .await?;
        }
        Ok(())
    }
}

/// All published messages are delivered in order, and the stream terminates once
/// the server-side handler returns.
#[tokio::test]
async fn test_service_stream_multiple_messages() {
    let server = nats_server::run_server("tests/nats/default.conf");

    let mut cluster = Cluster::new(server.client_url()).unwrap();
    cluster.register(TestServiceWithStreamMultipleImpl::service(()));
    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });
    sleep(Duration::from_millis(50)).await;

    let client =
        TestServiceWithStreamClient::new(async_nats::connect(server.client_url()).await.unwrap());
    let mut stream = client.test_stream().await.unwrap();

    assert_eq!(stream.next().await.unwrap().unwrap(), 10);
    assert_eq!(stream.next().await.unwrap().unwrap(), 20);
    assert_eq!(stream.next().await.unwrap().unwrap(), 30);
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Measurement {
    sensor_id: u32,
    value: f64,
}

#[service(name = "test_measurement_stream", version = "0.1.0")]
trait TestMeasurementStreamService {
    type Context;

    #[stream(
        name = "MEASUREMENT_STREAM",
        subject = "measurement",
        message = Measurement,
    )]
    async fn measurement_stream(ctx: StreamContext<Self::Context>) -> Result<(), Error>;
}

#[derive(Debug)]
struct TestMeasurementStreamImpl;

impl TestMeasurementStreamService for TestMeasurementStreamImpl {
    type Context = ();

    async fn measurement_stream(ctx: StreamContext<Self::Context>) -> Result<(), Error> {
        ctx.send(
            "measurement",
            &Measurement {
                sensor_id: 42,
                value: 98.6,
            },
        )
        .await?
        .await?;
        Ok(())
    }
}

#[tokio::test]
async fn test_service_stream_struct_message() {
    let server = nats_server::run_server("tests/nats/default.conf");

    let mut cluster = Cluster::new(server.client_url()).unwrap();
    cluster.register(TestMeasurementStreamImpl::service(()));
    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });
    sleep(Duration::from_millis(50)).await;

    let client = TestMeasurementStreamServiceClient::new(
        async_nats::connect(server.client_url()).await.unwrap(),
    );
    let mut stream = client.measurement_stream().await.unwrap();
    let msg = stream.next().await.unwrap().unwrap();

    assert_eq!(msg.sensor_id, 42);
    assert_eq!(msg.value, 98.6);
}
