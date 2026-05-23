use async_nats::service;
use rofr::ClientError;
use rofr::Cluster;
use rofr::Error;
use rofr::Request;
use rofr::RequestContext;
use rofr::Response;
use rofr::StreamContext;
use rofr::futures::StreamExt;
use rofr::service;
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Serialize, Deserialize)]
pub struct ExampleRequest {
    input: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExampleResponse {
    output: String,
}

#[service(name = "test_service", version = "0.1.2")]
trait TestService {
    type Context;

    #[endpoint(subject = "response_only")]
    async fn response_only(ctx: RequestContext<Self::Context>) -> Result<Response<()>, Error>;

    #[endpoint(subject = "echo")]
    async fn echo(
        _ctx: RequestContext<Self::Context>,
        body: Request<ExampleRequest>,
    ) -> Result<Response<ExampleResponse>, Error>;
}

#[derive(Debug)]
pub struct TestImpl;

impl TestService for TestImpl {
    type Context = ();

    async fn response_only(_ctx: RequestContext<Self::Context>) -> Result<Response<()>, Error> {
        Ok(Response(()))
    }

    async fn echo(
        _ctx: RequestContext<Self::Context>,
        body: Request<ExampleRequest>,
    ) -> Result<Response<ExampleResponse>, Error> {
        Ok(Response(ExampleResponse {
            output: body.input.to_owned(),
        }))
    }
}

#[tokio::test]
async fn test_service_info() {
    let server = nats_server::run_server("tests/nats/default.conf");
    let client = async_nats::connect(server.client_url()).await.unwrap();

    let mut cluster = Cluster::new(server.client_url()).unwrap();
    let test_service = TestImpl::service(());
    cluster.register(test_service);

    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    let info: service::Info = serde_json::from_slice(
        &client
            .request("$SRV.INFO", "".into())
            .await
            .unwrap()
            .payload,
    )
    .unwrap();

    assert_eq!(info.version, "0.1.2");
    assert_eq!(info.name, "test_service");
    assert_eq!(info.endpoints.len(), 2);
}

/// End-to-end test with a simple echo endpoint.
#[tokio::test]
async fn test_service_echo() {
    let server = nats_server::run_server("tests/nats/default.conf");
    let client = async_nats::connect(server.client_url()).await.unwrap();
    let client = TestServiceClient::new(client);

    let mut cluster = Cluster::new(server.client_url()).unwrap();
    let test_service = TestImpl::service(());
    cluster.register(test_service);

    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    let sample_input = "Example text goes in, example text goes out. You can't explain that.";

    let response = client
        .echo(ExampleRequest {
            input: sample_input.to_owned(),
        })
        .await
        .unwrap();

    assert_eq!(response.output, sample_input);
}

#[tokio::test]
async fn test_cluster_no_services() {
    let server = nats_server::run_server("tests/nats/default.conf");

    let cluster = Cluster::new(server.client_url()).unwrap();

    let result = tokio::time::timeout(Duration::from_millis(50), cluster.run()).await;
    assert!(
        result.is_err(),
        "cluster without services exited immediately"
    );
}

#[service(name = "test_service_no_endpoints", version = "0.1.2")]
trait TestServiceNoEndpoints {
    type Context;
}

#[derive(Debug)]
struct TestServiceNoEndpointsImpl;

impl TestServiceNoEndpoints for TestServiceNoEndpointsImpl {
    type Context = ();
}

#[tokio::test]
async fn test_serivce_no_endpoints() {
    let server = nats_server::run_server("tests/nats/default.conf");

    let mut cluster = Cluster::new(server.client_url()).unwrap();
    let test_service = TestServiceNoEndpointsImpl::service(());
    cluster.register(test_service);

    let result = tokio::time::timeout(Duration::from_millis(50), cluster.run()).await;
    assert!(
        result.is_err(),
        "cluster without services exited immediately"
    );
}

#[service(name = "test_service_with_stream", version = "0.1.2")]
trait TestServiceWithStream {
    type Context;

    #[stream(
        name = "TEST_STREAM",
        subject = "test_stream_subject",
        message = u64,
    )]
    async fn test_stream(ctx: StreamContext<Self::Context>) -> Result<(), Error>;
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
