use async_nats::service;
use nodal::Cluster;
use nodal::Error;
use nodal::Request;
use nodal::RequestContext;
use nodal::Response;
use nodal::service;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExampleRequest {
    input: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
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
