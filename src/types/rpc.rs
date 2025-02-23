use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a JSON-RPC request.
///
/// A JSON-RPC request consists of the `jsonrpc` version, the method being
/// called, any associated parameters (`params`), and a unique ID (`id`).
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcRequest {
    /// The JSON-RPC protocol version, typically "2.0".
    pub jsonrpc: String,
    /// The method name to be invoked.
    pub method: String,
    /// The parameters for the method as a JSON value.
    pub params: Value,
    /// The identifier for the request, used to match with a response.
    pub id: Value,
}

/// Represents a JSON-RPC response.
///
/// A JSON-RPC response contains the `jsonrpc` version, the result of the
/// requested operation (`result`), and the unique ID of the corresponding
/// request (`id`).
#[derive(Debug, Serialize)]
pub struct RpcResponse {
    /// The JSON-RPC protocol version, typically "2.0".
    pub jsonrpc: String,
    /// The result of the operation as a JSON value.
    pub result: Value,
    /// The identifier matching the corresponding request.
    pub id: Value,
}
