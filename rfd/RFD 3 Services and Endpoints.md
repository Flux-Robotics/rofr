---
date: 2026-04-16
authors: |
  @liamkinne
labels: architecture
state: draft
---

# Services and Endpoints

Services are collections of endpoints that share an internal `Context` between
them. This is how reusable RoFr APIs are defined. Because of how they are
implemented using NATS service infrastructure, they can be run either together,
spanning processes, or across separate computers without requiring any change
to the implementation.

The abstraction over NATS is intentionally thin. Users can debug systems with
the existing NATS CLI and associated tooling without an unwieldy amount of
obfuscation.

## Services

A trait is defined for each service using the `#[service]` procedural macro that
generates the server-side boiler plate for endpoints as well as a client
implementation.

### Shared Context Type

The API trait must have an associated `Context` type. This type contains the shared
context between endpoints (if there is any) and generally is used for all of the
stateful necessities that are required by the implementation.

### Example API Trait Definition

Here is an example weather station service API. It shows how endpoints can none
or one request body, and return scalar or complex values (so long as they derive
Serialize).

```rust
/// A request body which needs to derive serde traits so it can be used by the
/// server and client implementations.
#[derive(serde::Deserialize, serde::Serialize)]
pub struct SetInterval {
    interval_secs: f64,
}

/// Likewise request responses need to derive serde traits for the same reason.
#[derive(serde::Deserialize, serde::Serialize)]
pub struct WindSpeed {
    speed_kmh: f64,
    direction_deg: f64,
}

#[service(name = "weather_station", version = "0.1.0")]
trait WeatherStationService {
    type Context;

    #[endpoint(subject = "wind_speed")]
    async fn wind_speed(ctx: RequestContext<Self::Context>) -> Result<Response<WindSpeed>, Error>;

    #[endpoint(subject = "set_interval")]
    async fn set_interval(
        ctx: RequestContext<Self::Context>,
        body: Request<SetInterval>,
    ) -> Result<Response<()>, Error>;
}
```

### Service Implementations

Creating an implementation of service API is the same as implementing any other
Rust trait. The function bodies are filled out with code and the final type for
the `Context` type is provided.

```rust
/// Shared state context type. Can be specific to this implementation or re-used
/// for other implementations.
pub struct WeatherContext {
    interval_secs: f64
}

/// Empty type for the implementation to use.
pub struct WeatherStation;

impl WeatherStationService for WeatherStation {
    type Context = WeatherContext;

    async fn wind_speed(ctx: RequestContext<Self::Context>) -> Result<Response<WindSpeed>, Error> {
        Ok(Response(WindSpeed { speed_kmh: 5.0, direction_deg: 65.0 }))
    }

    async fn set_interval(
        ctx: RequestContext<Self::Context>,
        body: Request<SetInterval>,
    ) -> Result<Response<()>, Error> {
        Ok(Response(()))
    }
}
```

### Service Parameters

Users need to be able to run multiple instances of a service and distinguish
between them. This is done by adding one or more parameters onto the end of the
service name of which values can be chosen at runtime.

NATS doesn't allow the `.` character in service names so instead the parameters
are prepended to the endpoint name. The final endpoint NATS subject is of the
form `<service-name>.<parameters>.<endpoint-name>`.

### Service Versions

Services must specify a version (as required by NATS) which allows keeping track
of API changes and client compatibility.

As specified by [SemVer](https://semver.org/), the major version indicates
breaking API changes, the minor version indicates new functionality that is
backwards compatible, and the patch version indicates backwards compatible bug
fixes.

Clients can check if they are compatible with the server at runtime by comparing
version numbers.

## Endpoints

Endpoints provide a request-reply interface with a body and a response.

```rust
#[endpoint(subject = "foo")]
async fn foo(
    ctx: RequestContext<Self::Context>,
    body: Request<Bar>,
) -> Result<Response<Baz>, Error>;
```

The request and response type must implement serde's `Deserialize` and
`Serialize` traits.

### Request Context

The request context type provides access to the shared context, a NATS client with the same connection as the server, and per-request information like the `request_id`.

### Errors and Success

The endpoint handler returns a `Result<T, E>` for implementations to signal a
failure to process the request.

### Parallel Execution

Endpoint handlers can run in parallel which is why the context type is shared
immutably between handlers. Any locking required for mutable data inside the
context type is left for the user to provide.
