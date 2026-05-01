use async_nats::jetstream::stream::StorageType;
use rofr::Cluster;
use rofr::Error;
use rofr::Request;
use rofr::RequestContext;
use rofr::Response;
use rofr::StreamContext;
use rofr::service;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;

/// Interval configuration request body.
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SetInterval {
    interval_secs: f64,
}

/// Temperature reading.
#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct Temperature {
    degrees_celsius: f64,
}

/// Weather station service with two parameters, `location` and `id`.
#[service(name = "weather.{location}.{id}", version = "0.1.0")]
trait WeatherService {
    type Context;

    #[stream(
        name = "TEMPERATURE",
        subject = "temperature",
        storage = StorageType::Memory,
        message = Temperature,
    )]
    async fn temperature(ctx: StreamContext<Self::Context>) -> Result<(), Error>;

    /// Example of a response-only endpoint that requires no body to be provided
    /// in the request.
    #[endpoint(subject = "wind_speed")]
    async fn wind_speed(ctx: RequestContext<Self::Context>) -> Result<Response<f64>, Error>;

    /// Request-response endpoint that mutates the service context.
    #[endpoint(subject = "set_interval")]
    async fn set_interval(
        ctx: RequestContext<Self::Context>,
        body: Request<SetInterval>,
    ) -> Result<Response<()>, Error>;

    /// Request the interval value.
    #[endpoint(subject = "interval")]
    async fn interval(ctx: RequestContext<Self::Context>) -> Result<Response<f64>, Error>;
}

/// Shared weather service context. Automatically wrapped in a mutex by the
/// framework.
pub struct WeatherContext {
    interval: Arc<Mutex<std::time::Duration>>,
}

pub enum WeatherImpl {}

impl WeatherService for WeatherImpl {
    type Context = WeatherContext;

    async fn temperature(ctx: StreamContext<Self::Context>) -> Result<(), Error> {
        loop {
            ctx.send(
                "temperature",
                &Temperature {
                    degrees_celsius: 22.5,
                },
            )
            .await? // publish to NATS
            .await?; // wait for ack from NATS

            let interval = ctx.context().interval.lock().await.to_owned();
            tokio::time::sleep(interval).await;
        }
    }

    async fn wind_speed(_ctx: RequestContext<WeatherContext>) -> Result<Response<f64>, Error> {
        let speed: f64 = rand::random();
        Ok(Response(speed))
    }

    async fn set_interval(
        ctx: RequestContext<WeatherContext>,
        body: Request<SetInterval>,
    ) -> Result<Response<()>, Error> {
        *ctx.context().interval.lock().await = Duration::from_secs_f64(body.interval_secs);
        tracing::info!("Interval set to {} seconds", body.interval_secs);
        Ok(Response(()))
    }

    async fn interval(ctx: RequestContext<WeatherContext>) -> Result<Response<f64>, Error> {
        Ok(Response(ctx.context().interval.lock().await.as_secs_f64()))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    // service internal state
    let weather_ctx = WeatherContext {
        interval: Arc::new(Mutex::new(Duration::from_secs(60))),
    };

    // create a cluster with a NATS connection address
    let mut cluster = Cluster::new("localhost:4222")?;

    // service parameters
    let params = ("virginia", "abc");

    // register a service with the cluster
    cluster.register(WeatherImpl::service(weather_ctx, params));

    // spawn cluster in background
    tokio::spawn(async move {
        cluster.run().await.unwrap();
    });

    let nats = async_nats::connect("localhost:4222").await?;
    let client = WeatherServiceClient::new(nats, params);

    loop {
        sleep(Duration::from_secs(1)).await;
        let speed = client.wind_speed().await?;
        println!("Wind speed: {:.02} m/s", speed);
    }
}
