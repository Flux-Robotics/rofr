use std::fmt;

/// Client error.
#[derive(Debug)]
pub enum ClientError {
    Serialize(serde_json::Error),
    Request(Box<dyn std::error::Error + Send + Sync>),
    Deserialize(serde_json::Error),
    ServiceError(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Serialize(e) => write!(f, "serialization error: {e}"),
            ClientError::Request(e) => write!(f, "request error: {e}"),
            ClientError::Deserialize(e) => write!(f, "deserialization error: {e}"),
            ClientError::ServiceError(msg) => write!(f, "service error: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Generates a new unique request id.
pub fn generate_request_id() -> String {
    ulid::Ulid::new().to_string()
}
