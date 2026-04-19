//! Header keys used in various NATS metadata.

/// Service Nodal version header.
pub const VERSION: &str = "Nodal-Version";
/// Service unique identifier header.
pub const SERVICE_UID: &str = "Nodal-Service-Uid";
/// Endpoint request id header.
pub const REQUEST_ID: &str = "Nodal-Request-Id";
/// Endpoint request id header.
pub const MESSAGE_ID: &str = "Nodal-Message-Id";
/// Endpoint request schema header.
pub const REQUEST_SCHEMA: &str = "Nodal-Request-Schema";
/// Endpoint response schema header.
pub const RESPONSE_SCHEMA: &str = "Nodal-Response-Schema";
/// Stream message schema header.
pub const MESSAGE_SCHEMA: &str = "Nodal-Message-Schema";
