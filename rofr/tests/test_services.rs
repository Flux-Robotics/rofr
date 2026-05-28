use async_nats::service;
use rofr::ClientError;
use rofr::Cluster;
use rofr::Error;
use rofr::Request;
use rofr::RequestContext;
use rofr::Response;
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
        ctx: RequestContext<Self::Context>,
        body: Request<ExampleRequest>,
    ) -> Result<Response<ExampleResponse>, Error>;

    #[endpoint(subject = "return_error")]
    async fn return_error(ctx: RequestContext<Self::Context>) -> Result<Response<()>, Error>;
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

    async fn return_error(_ctx: RequestContext<Self::Context>) -> Result<Response<()>, Error> {
        Err(Error::new("example error message"))
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
    assert_eq!(info.endpoints.len(), 3);
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
async fn test_service_endpoint_error_response() {
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

    let result = client
        .return_error()
        .await
        .expect_err("expected error response");

    matches!(result, ClientError::ServiceError(_));
}

#[service(name = "test_context_service", version = "0.1.0")]
trait TestContextService {
    type Context;

    #[endpoint(subject = "read_context")]
    async fn read_context(ctx: RequestContext<Self::Context>) -> Result<Response<String>, Error>;
}

#[derive(Debug)]
struct TestContextServiceImpl;

impl TestContextService for TestContextServiceImpl {
    type Context = String;

    async fn read_context(ctx: RequestContext<Self::Context>) -> Result<Response<String>, Error> {
        let value = ctx.context().clone();
        let _ = ctx.nats();
        Ok(Response(value))
    }
}

#[tokio::test]
async fn test_request_context_methods() {
    let server = nats_server::run_server("tests/nats/default.conf");
    let client = async_nats::connect(server.client_url()).await.unwrap();
    let rpc_client = TestContextServiceClient::new(client);

    let context_value = "hello-from-context".to_string();
    let mut cluster = Cluster::new(server.client_url()).unwrap();
    cluster.register(TestContextServiceImpl::service(context_value.clone()));
    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    let result = rpc_client.read_context().await.unwrap();
    assert_eq!(result, context_value);
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
        "service without endpoints exited immediately"
    );
}

#[service(name = "robot.{id}", version = "0.1.0")]
trait RobotService {
    type Context;

    #[endpoint(subject = "ping")]
    async fn ping(ctx: RequestContext<Self::Context>) -> Result<Response<String>, Error>;
}

#[derive(Debug)]
struct RobotServiceImpl;

impl RobotService for RobotServiceImpl {
    type Context = ();

    async fn ping(_ctx: RequestContext<Self::Context>) -> Result<Response<String>, Error> {
        Ok(Response("pong".to_string()))
    }
}

#[tokio::test]
async fn test_template_service_endpoint() {
    let server = nats_server::run_server("tests/nats/default.conf");
    let client = async_nats::connect(server.client_url()).await.unwrap();

    let robot_id = "robot-42";
    let client = RobotServiceClient::new(client, (robot_id,));

    let mut cluster = Cluster::new(server.client_url()).unwrap();
    cluster.register(RobotServiceImpl::service((), (robot_id,)));
    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    let result = client.ping().await.unwrap();
    assert_eq!(result, "pong");
}
