use alloy::primitives::U256;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Helper function to provide default empty JSON array for optional params
/// This ensures backward compatibility when params field is missing from wallet requests
fn default_empty_params() -> Value {
    Value::Array(vec![])
}

pub const NONCE_SIZE: usize = 4;

/// Represents a JSON-RPC request.
///
/// A JSON-RPC request consists of the `jsonrpc` version, the method being
/// called, optional parameters (`params`), and a unique ID (`id`).
///
/// The `params` field is optional to support wallet compatibility. Many wallets
/// (like MetaMask) omit the params field for methods that don't require parameters.
/// When missing, it defaults to an empty JSON array for backward compatibility.
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcRequest {
    /// The JSON-RPC protocol version, typically "2.0".
    pub jsonrpc: String,
    /// The method name to be invoked.
    pub method: String,
    /// The parameters for the method as a JSON value.
    /// Optional to support wallet compatibility - defaults to empty array when missing.
    #[serde(default = "default_empty_params")]
    pub params: Value,
    /// The identifier for the request, used to match with a response.
    pub id: Value,
}

/// Envelope that accepts either a single JSON-RPC request object or a batch (array) of requests.
///
/// Using untagged deserialization allows serde to choose the variant based on the JSON shape
/// (object vs array), which matches JSON-RPC 2.0 behavior for single vs batch requests.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcEnvelope {
    Single(RpcRequest),
    Batch(Vec<RpcRequest>),
}

/// Represents the type of an IGRA L2 transaction.
///
/// This enum is used in the L1 payload to identify the kind of L2 data being transmitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)] // Some variants not yet implemented
pub enum TxTypeId {
    /// L2 Start transaction (0x00)
    L2Start = 0x00,
    /// Entry transaction (0x02)
    Entry = 0x02,
    /// 1-to-1 Unzipped Payload transaction (0x04)
    UnzippedPayload = 0x04,
    /// 1-to-1 Zipped Payload transaction (0x05)
    ZippedPayload = 0x05,
}

/// Represents the new IGRA L1 transaction payload format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgraPayload {
    /// The payload format version, fixed at `0x9`.
    pub version: u8,
    /// The type of L2 transaction.
    pub tx_type_id: TxTypeId,
    /// The L2-specific data.
    pub l2_data: Vec<u8>,
    /// The nonce used for mining a valid transaction ID.
    pub nonce: [u8; NONCE_SIZE],
}

#[derive(Serialize, Deserialize, Debug)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    pub id: Value,
    pub result: T,
}

/// Represents an Ethereum block for parsing eth_getBlockByNumber responses.
/// Contains only the fields we need for gas price calculations.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Block {
    /// The base fee per gas for this block (EIP-1559)
    pub base_fee_per_gas: Option<U256>,
    /// Block number (unused, optional)
    pub number: Option<U256>,
    /// Block hash (unused, optional)
    pub hash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Test fixtures and utilities
    struct TestRequest {
        method: &'static str,
        params: Option<Value>,
        id: i32,
    }

    impl TestRequest {
        fn new(method: &'static str, id: i32) -> Self {
            Self {
                method,
                params: None,
                id,
            }
        }

        fn with_params(mut self, params: Value) -> Self {
            self.params = Some(params);
            self
        }

        fn to_json_string(&self) -> String {
            match &self.params {
                Some(Value::Null) => {
                    format!(
                        r#"{{"jsonrpc":"2.0","method":"{}","params":null,"id":{}}}"#,
                        self.method, self.id
                    )
                }
                Some(params) => {
                    format!(
                        r#"{{"jsonrpc":"2.0","method":"{}","params":{},"id":{}}}"#,
                        self.method, params, self.id
                    )
                }
                None => {
                    format!(
                        r#"{{"jsonrpc":"2.0","method":"{}","id":{}}}"#,
                        self.method, self.id
                    )
                }
            }
        }

        fn to_rpc_request(&self) -> RpcRequest {
            serde_json::from_str(&self.to_json_string()).expect("Failed to parse test JSON")
        }
    }

    fn assert_request_basics(request: &RpcRequest, expected_method: &str, expected_id: i32) {
        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, expected_method);
        assert_eq!(request.id, json!(expected_id));
    }

    fn assert_empty_params(request: &RpcRequest) {
        assert_eq!(request.params, Value::Array(vec![]));
    }

    #[test]
    fn test_rpc_request_without_params() {
        // Test deserialization of wallet request without params field (MetaMask, Rabby)
        let request = TestRequest::new("eth_gasPrice", 1).to_rpc_request();

        assert_request_basics(&request, "eth_gasPrice", 1);
        assert_empty_params(&request);
    }

    #[test]
    fn test_rpc_request_with_empty_params() {
        // Test deserialization with empty params array
        let request = TestRequest::new("net_version", 2)
            .with_params(json!([]))
            .to_rpc_request();

        assert_request_basics(&request, "net_version", 2);
        assert_empty_params(&request);
    }

    #[test]
    fn test_rpc_request_with_populated_params() {
        // Test backward compatibility with existing params
        let params = json!(["0x407d73d8a49eeb85d32cf465507dd71d507100c1", "latest"]);
        let request = TestRequest::new("eth_getBalance", 3)
            .with_params(params.clone())
            .to_rpc_request();

        assert_request_basics(&request, "eth_getBalance", 3);
        assert_eq!(request.params, params);
    }

    #[test]
    fn test_rpc_request_with_null_params() {
        // Test handling of null params (should remain as null)
        let request = TestRequest::new("eth_chainId", 4)
            .with_params(Value::Null)
            .to_rpc_request();

        assert_request_basics(&request, "eth_chainId", 4);
        assert_eq!(request.params, Value::Null);
    }

    #[test]
    fn test_rpc_request_serialization() {
        // Test that serialization works correctly
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "eth_gasPrice".to_string(),
            params: Value::Array(vec![]),
            id: json!(1),
        };

        let serialized = serde_json::to_string(&request).expect("Failed to serialize request");
        let deserialized: RpcRequest =
            serde_json::from_str(&serialized).expect("Failed to deserialize request");

        // Use helper to verify basic fields
        assert_request_basics(&deserialized, "eth_gasPrice", 1);
        assert_empty_params(&deserialized);
    }

    #[test]
    fn test_rpc_envelope_single_without_params() {
        // Test envelope with single request without params
        let json_str = TestRequest::new("eth_gasPrice", 1).to_json_string();
        let envelope: RpcEnvelope =
            serde_json::from_str(&json_str).expect("Failed to parse envelope JSON");

        match envelope {
            RpcEnvelope::Single(request) => {
                assert_request_basics(&request, "eth_gasPrice", 1);
                assert_empty_params(&request);
            }
            _ => panic!("Expected single request"),
        }
    }

    #[test]
    fn test_rpc_envelope_batch_mixed_params() {
        // Test batch with mixed params scenarios
        let req1 = TestRequest::new("eth_gasPrice", 1).to_json_string();
        let req2 = TestRequest::new("eth_getBalance", 2)
            .with_params(json!([
                "0x407d73d8a49eeb85d32cf465507dd71d507100c1",
                "latest"
            ]))
            .to_json_string();
        let batch_json = format!("[{req1},{req2}]");

        let envelope: RpcEnvelope =
            serde_json::from_str(&batch_json).expect("Failed to parse batch JSON");

        match envelope {
            RpcEnvelope::Batch(requests) => {
                assert_eq!(requests.len(), 2);

                assert_request_basics(&requests[0], "eth_gasPrice", 1);
                assert_empty_params(&requests[0]);

                assert_request_basics(&requests[1], "eth_getBalance", 2);
                assert_eq!(
                    requests[1].params,
                    json!(["0x407d73d8a49eeb85d32cf465507dd71d507100c1", "latest"])
                );
            }
            _ => panic!("Expected batch request"),
        }
    }

    #[test]
    fn test_wallet_compatibility_requests() {
        // Test specific wallet scenarios that were causing issues
        let wallet_scenarios = [
            ("eth_gasPrice", "MetaMask"),
            ("net_version", "Rabby"),
            ("eth_chainId", "Generic wallet"),
        ];

        for (i, (method, wallet_type)) in wallet_scenarios.iter().enumerate() {
            let id = i32::try_from(i).expect("Index too large for i32") + 1;
            let request = TestRequest::new(method, id).to_rpc_request();

            assert_request_basics(&request, method, id);
            assert_empty_params(&request);

            // Additional verification that the wallet type scenario works
            println!("✓ {wallet_type} request compatibility verified");
        }
    }

    #[test]
    fn test_batch_request_with_mixed_param_presence() {
        // Test a realistic batch request where some methods have params, others don't
        let batch_requests = [
            TestRequest::new("eth_gasPrice", 1),
            TestRequest::new("eth_getBalance", 2)
                .with_params(json!(["0x1234567890abcdef", "latest"])),
            TestRequest::new("net_version", 3),
        ];

        let batch_json = format!(
            "[{}]",
            batch_requests
                .iter()
                .map(|req| req.to_json_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let envelope: RpcEnvelope =
            serde_json::from_str(&batch_json).expect("Failed to parse batch JSON");

        match envelope {
            RpcEnvelope::Batch(requests) => {
                assert_eq!(requests.len(), 3);

                // First request: no params
                assert_request_basics(&requests[0], "eth_gasPrice", 1);
                assert_empty_params(&requests[0]);

                // Second request: has params
                assert_request_basics(&requests[1], "eth_getBalance", 2);
                assert_eq!(requests[1].params, json!(["0x1234567890abcdef", "latest"]));

                // Third request: no params
                assert_request_basics(&requests[2], "net_version", 3);
                assert_empty_params(&requests[2]);
            }
            _ => panic!("Expected batch request"),
        }
    }
}
