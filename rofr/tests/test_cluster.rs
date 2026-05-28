use std::time::Duration;

use rofr::Cluster;

#[tokio::test]
async fn cluster_no_services() {
    let server = nats_server::run_server("tests/nats/default.conf");

    let cluster = Cluster::new(server.client_url()).unwrap();

    let result = tokio::time::timeout(Duration::from_millis(50), cluster.run()).await;
    assert!(
        result.is_err(),
        "cluster without services exited immediately"
    );
}
