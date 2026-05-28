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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_serde_error() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("invalid json {{{").unwrap_err()
    }

    #[test]
    fn client_error_display_serialize() {
        let err = ClientError::Serialize(make_serde_error());
        assert!(err.to_string().starts_with("serialization error:"));
    }

    #[test]
    fn client_error_display_deserialize() {
        let err = ClientError::Deserialize(make_serde_error());
        assert!(err.to_string().starts_with("deserialization error:"));
    }

    #[test]
    fn client_error_display_request() {
        let inner: Box<dyn std::error::Error + Send + Sync> =
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, "timeout"));
        let err = ClientError::Request(inner);
        assert_eq!(err.to_string(), "request error: timeout");
    }

    #[test]
    fn client_error_display_service_error() {
        let err = ClientError::ServiceError("something went wrong".to_string());
        assert_eq!(err.to_string(), "service error: something went wrong");
    }

    #[test]
    fn client_error_debug_impl() {
        let err = ClientError::ServiceError("oops".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("ServiceError"));
    }

    #[test]
    fn test_generate_request_id_length() {
        assert_eq!(generate_request_id().len(), 26);
    }

    #[test]
    fn test_generate_request_id_unique() {
        let a = generate_request_id();
        let b = generate_request_id();
        assert_ne!(a, b);
    }
}
