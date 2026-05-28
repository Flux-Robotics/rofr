use std::time::Duration;

use async_nats::ConnectOptions;
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

#[tokio::test]
async fn cluster_with_connect_options() {
    let server = nats_server::run_server("tests/nats/default.conf");

    let options = ConnectOptions::new().name("rofr-cluster-with-connect-options");
    let cluster = Cluster::new_with_options(server.client_url(), options).unwrap();

    let result = tokio::time::timeout(Duration::from_millis(50), cluster.run()).await;
    assert!(
        result.is_err(),
        "cluster with connect options exited immediately"
    );
}
