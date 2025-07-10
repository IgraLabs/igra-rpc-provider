/// Proxy service domain-specific errors
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// Request serialization failed
    #[error("Request serialization failed: {0}")]
    SerializationFailed(String),

    /// Request deserialization failed
    #[error("Request deserialization failed: {0}")]
    DeserializationFailed(String),

    /// EL client communication failed
    #[error("EL client communication failed: {0}")]
    ElCommunicationFailed(String),

    /// EL client connection failed
    #[error("EL client connection failed: url={url}, reason={reason}")]
    ElConnectionFailed { url: String, reason: String },

    /// EL client timeout
    #[error("EL client request timeout: url={url}, timeout_seconds={timeout_seconds}")]
    ElTimeout { url: String, timeout_seconds: u64 },

    /// Invalid EL URL
    #[error("Invalid EL URL: {url}, reason={reason}")]
    InvalidElUrl { url: String, reason: String },

    /// EL client returned an error
    #[error("EL client error: code={code}, message={message}")]
    ElClientError { code: i32, message: String },

    /// Unsupported RPC method
    #[error("Unsupported RPC method: {method}")]
    UnsupportedMethod { method: String },

    /// Invalid RPC request format
    #[error("Invalid RPC request format: {0}")]
    InvalidRequestFormat(String),

    /// Invalid RPC response format
    #[error("Invalid RPC response format: {0}")]
    InvalidResponseFormat(String),

    /// Request routing failed
    #[error("Request routing failed: method={method}, reason={reason}")]
    RoutingFailed { method: String, reason: String },

    /// Response transformation failed
    #[error("Response transformation failed: {0}")]
    ResponseTransformationFailed(String),

    /// Request validation failed
    #[error("Request validation failed: {0}")]
    RequestValidationFailed(String),

    /// Rate limiting exceeded
    #[error("Rate limit exceeded: {limit} requests per {window_seconds} seconds")]
    RateLimitExceeded { limit: u32, window_seconds: u32 },

    /// Proxy service unavailable
    #[error("Proxy service temporarily unavailable")]
    ServiceUnavailable,

    /// Request too large
    #[error("Request too large: size={size} bytes, max_allowed={max_allowed} bytes")]
    RequestTooLarge { size: usize, max_allowed: usize },

    /// Response too large
    #[error("Response too large: size={size} bytes, max_allowed={max_allowed} bytes")]
    ResponseTooLarge { size: usize, max_allowed: usize },

    /// Circuit breaker open
    #[error("Circuit breaker open for EL client: {url}")]
    CircuitBreakerOpen { url: String },

    /// Load balancer error
    #[error("Load balancer error: {0}")]
    LoadBalancerError(String),

    /// Configuration error
    #[error("Proxy configuration error: {0}")]
    ConfigurationError(String),

    /// Internal proxy error
    #[error("Internal proxy error: {0}")]
    InternalError(String),
}

impl ProxyError {
    /// Create an EL communication error
    pub fn el_communication_failed(reason: impl Into<String>) -> Self {
        Self::ElCommunicationFailed(reason.into())
    }

    /// Create an EL connection error
    pub fn el_connection_failed(url: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ElConnectionFailed {
            url: url.into(),
            reason: reason.into(),
        }
    }

    /// Create an EL timeout error
    pub fn el_timeout(url: impl Into<String>, timeout_seconds: u64) -> Self {
        Self::ElTimeout {
            url: url.into(),
            timeout_seconds,
        }
    }

    /// Create an invalid EL URL error
    pub fn invalid_el_url(url: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidElUrl {
            url: url.into(),
            reason: reason.into(),
        }
    }

    /// Create an EL client error
    pub fn el_client_error(code: i32, message: impl Into<String>) -> Self {
        Self::ElClientError {
            code,
            message: message.into(),
        }
    }

    /// Create an unsupported method error
    pub fn unsupported_method(method: impl Into<String>) -> Self {
        Self::UnsupportedMethod {
            method: method.into(),
        }
    }

    /// Create a routing failed error
    pub fn routing_failed(method: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::RoutingFailed {
            method: method.into(),
            reason: reason.into(),
        }
    }

    /// Create a rate limit exceeded error
    pub fn rate_limit_exceeded(limit: u32, window_seconds: u32) -> Self {
        Self::RateLimitExceeded {
            limit,
            window_seconds,
        }
    }

    /// Create a request too large error
    pub fn request_too_large(size: usize, max_allowed: usize) -> Self {
        Self::RequestTooLarge { size, max_allowed }
    }

    /// Create a response too large error
    pub fn response_too_large(size: usize, max_allowed: usize) -> Self {
        Self::ResponseTooLarge { size, max_allowed }
    }

    /// Create a circuit breaker open error
    pub fn circuit_breaker_open(url: impl Into<String>) -> Self {
        Self::CircuitBreakerOpen { url: url.into() }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProxyError::ElCommunicationFailed(_)
                | ProxyError::ElConnectionFailed { .. }
                | ProxyError::ElTimeout { .. }
                | ProxyError::ServiceUnavailable
                | ProxyError::CircuitBreakerOpen { .. }
                | ProxyError::LoadBalancerError(_)
                | ProxyError::InternalError(_)
        )
    }

    /// Check if this error is a client error (4xx equivalent)
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            ProxyError::InvalidRequestFormat(_)
                | ProxyError::RequestValidationFailed(_)
                | ProxyError::UnsupportedMethod { .. }
                | ProxyError::RequestTooLarge { .. }
                | ProxyError::RateLimitExceeded { .. }
        )
    }

    /// Check if this error is a server error (5xx equivalent)
    pub fn is_server_error(&self) -> bool {
        matches!(
            self,
            ProxyError::ElCommunicationFailed(_)
                | ProxyError::ElConnectionFailed { .. }
                | ProxyError::ServiceUnavailable
                | ProxyError::InternalError(_)
                | ProxyError::LoadBalancerError(_)
                | ProxyError::ResponseTransformationFailed(_)
        )
    }

    /// Check if this error is a configuration error
    pub fn is_configuration_error(&self) -> bool {
        matches!(
            self,
            ProxyError::InvalidElUrl { .. } | ProxyError::ConfigurationError(_)
        )
    }

    /// Get error code for categorization
    pub fn error_code(&self) -> &'static str {
        match self {
            ProxyError::SerializationFailed(_) => "SERIALIZATION_FAILED",
            ProxyError::DeserializationFailed(_) => "DESERIALIZATION_FAILED",
            ProxyError::ElCommunicationFailed(_) => "EL_COMMUNICATION_FAILED",
            ProxyError::ElConnectionFailed { .. } => "EL_CONNECTION_FAILED",
            ProxyError::ElTimeout { .. } => "EL_TIMEOUT",
            ProxyError::InvalidElUrl { .. } => "INVALID_EL_URL",
            ProxyError::ElClientError { .. } => "EL_CLIENT_ERROR",
            ProxyError::UnsupportedMethod { .. } => "UNSUPPORTED_METHOD",
            ProxyError::InvalidRequestFormat(_) => "INVALID_REQUEST_FORMAT",
            ProxyError::InvalidResponseFormat(_) => "INVALID_RESPONSE_FORMAT",
            ProxyError::RoutingFailed { .. } => "ROUTING_FAILED",
            ProxyError::ResponseTransformationFailed(_) => "RESPONSE_TRANSFORMATION_FAILED",
            ProxyError::RequestValidationFailed(_) => "REQUEST_VALIDATION_FAILED",
            ProxyError::RateLimitExceeded { .. } => "RATE_LIMIT_EXCEEDED",
            ProxyError::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            ProxyError::RequestTooLarge { .. } => "REQUEST_TOO_LARGE",
            ProxyError::ResponseTooLarge { .. } => "RESPONSE_TOO_LARGE",
            ProxyError::CircuitBreakerOpen { .. } => "CIRCUIT_BREAKER_OPEN",
            ProxyError::LoadBalancerError(_) => "LOAD_BALANCER_ERROR",
            ProxyError::ConfigurationError(_) => "CONFIGURATION_ERROR",
            ProxyError::InternalError(_) => "INTERNAL_ERROR",
        }
    }

    /// Get HTTP status code equivalent
    pub fn http_status_code(&self) -> u16 {
        match self {
            ProxyError::InvalidRequestFormat(_)
            | ProxyError::RequestValidationFailed(_)
            | ProxyError::UnsupportedMethod { .. }
            | ProxyError::RequestTooLarge { .. } => 400, // Bad Request

            ProxyError::RateLimitExceeded { .. } => 429, // Too Many Requests

            ProxyError::ElCommunicationFailed(_)
            | ProxyError::ElConnectionFailed { .. }
            | ProxyError::ServiceUnavailable
            | ProxyError::InternalError(_)
            | ProxyError::LoadBalancerError(_)
            | ProxyError::ResponseTransformationFailed(_) => 502, // Bad Gateway

            ProxyError::ElTimeout { .. } => 504, // Gateway Timeout

            ProxyError::CircuitBreakerOpen { .. } => 503, // Service Unavailable

            ProxyError::ResponseTooLarge { .. } => 502, // Bad Gateway

            _ => 500, // Internal Server Error
        }
    }

    /// Get suggested retry delay in seconds for retryable errors
    pub fn retry_delay_seconds(&self) -> Option<u64> {
        match self {
            ProxyError::ElCommunicationFailed(_) => Some(1),
            ProxyError::ElConnectionFailed { .. } => Some(2),
            ProxyError::ElTimeout { .. } => Some(1),
            ProxyError::ServiceUnavailable => Some(5),
            ProxyError::CircuitBreakerOpen { .. } => Some(10),
            ProxyError::LoadBalancerError(_) => Some(2),
            ProxyError::InternalError(_) => Some(3),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_el_communication_error() {
        let error = ProxyError::el_communication_failed("Connection refused");
        assert!(error.is_retryable());
        assert!(error.is_server_error());
        assert!(!error.is_client_error());
        assert_eq!(error.error_code(), "EL_COMMUNICATION_FAILED");
        assert_eq!(error.http_status_code(), 502);
        assert_eq!(error.retry_delay_seconds(), Some(1));
    }

    #[test]
    fn test_unsupported_method_error() {
        let error = ProxyError::unsupported_method("eth_unsupportedMethod");
        assert!(!error.is_retryable());
        assert!(error.is_client_error());
        assert!(!error.is_server_error());
        assert_eq!(error.error_code(), "UNSUPPORTED_METHOD");
        assert_eq!(error.http_status_code(), 400);
    }

    #[test]
    fn test_rate_limit_exceeded_error() {
        let error = ProxyError::rate_limit_exceeded(100, 60);
        assert!(!error.is_retryable());
        assert!(error.is_client_error());
        assert_eq!(error.error_code(), "RATE_LIMIT_EXCEEDED");
        assert_eq!(error.http_status_code(), 429);
    }

    #[test]
    fn test_el_timeout_error() {
        let error = ProxyError::el_timeout("http://localhost:8545", 30);
        assert!(error.is_retryable());
        assert!(!error.is_client_error());
        assert_eq!(error.error_code(), "EL_TIMEOUT");
        assert_eq!(error.http_status_code(), 504);
        assert_eq!(error.retry_delay_seconds(), Some(1));
    }

    #[test]
    fn test_invalid_el_url_error() {
        let error = ProxyError::invalid_el_url("invalid-url", "Invalid scheme");
        assert!(!error.is_retryable());
        assert!(error.is_configuration_error());
        assert_eq!(error.error_code(), "INVALID_EL_URL");
        assert_eq!(error.http_status_code(), 500);
    }

    #[test]
    fn test_circuit_breaker_open_error() {
        let error = ProxyError::circuit_breaker_open("http://localhost:8545");
        assert!(error.is_retryable());
        assert_eq!(error.error_code(), "CIRCUIT_BREAKER_OPEN");
        assert_eq!(error.http_status_code(), 503);
        assert_eq!(error.retry_delay_seconds(), Some(10));
    }

    #[test]
    fn test_request_too_large_error() {
        let error = ProxyError::request_too_large(2048, 1024);
        assert!(!error.is_retryable());
        assert!(error.is_client_error());
        assert_eq!(error.error_code(), "REQUEST_TOO_LARGE");
        assert_eq!(error.http_status_code(), 400);
    }
}
