use async_nats::service;
use nodal::{Cluster, Error, RequestContext, Response, service};
use std::time::Duration;
use tokio::time::sleep;

#[service(name = "test_service", version = "0.1.2")]
trait TestService {
    type Context;

    #[endpoint(subject = "response_only")]
    async fn response_only(ctx: RequestContext<Self::Context>) -> Result<Response<()>, Error>;
}

#[derive(Debug)]
pub struct TestImpl;

impl TestService for TestImpl {
    type Context = ();

    async fn response_only(_ctx: RequestContext<Self::Context>) -> Result<Response<()>, Error> {
        Ok(Response(()))
    }
}

#[tokio::test]
async fn test_service() {
    let server = nats_server::run_server("tests/nats/default.conf");
    let client = async_nats::connect(server.client_url()).await.unwrap();

    let mut cluster = Cluster::new(server.client_url()).unwrap();

    let test_service = TestImpl::service(());

    cluster.register(test_service);

    let cluster_task = tokio::spawn(async move {
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
    assert_eq!(info.endpoints.len(), 1);
    assert_eq!(info.endpoints[0].name, "test_service-response_only");
    assert_eq!(info.endpoints[0].subject, "test_service.response_only");

    cluster_task.abort();
}
