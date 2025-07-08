use crate::clients::el_caller;
use crate::config::GasConfig;
use crate::error::AppError;
use crate::types::rpc::{Block, JsonRpcResponse};
use ethers::types::U256;
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
use tracing::info;

const GWEI_TO_WEI: U256 = U256([1_000_000_000, 0, 0, 0]);
const IGRA_BLOCK_TIME: u64 = 1;

/// A service to handle gas price logic, such as enforcing a minimum floor.
#[derive(Debug, Clone)]
pub struct GasPriceService {
    config: GasConfig,
    // Cache the last computed effective base fee together with the moment it was fetched.
    // Shared through an Arc so all cloned instances see the same cache.
    cache: std::sync::Arc<tokio::sync::RwLock<Option<CachedFee>>>,
}

// Holds a cached fee value and the instant it was obtained.
#[derive(Debug, Clone)]
struct CachedFee {
    fee: U256,
    fetched_at: std::time::Instant,
}

impl GasPriceService {
    /// Creates a new `GasPriceService`.
    pub fn new(config: GasConfig) -> Self {
        Self {
            config,
            cache: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    /// Helper: calculate the configured minimum floor in Wei using checked arithmetic.
    fn min_floor_wei(&self) -> Result<U256, AppError> {
        let gwei = U256::from(self.config.min_base_fee_gwei);
        gwei.checked_mul(GWEI_TO_WEI).ok_or_else(|| {
            AppError::Internal("min_base_fee_gwei multiplication overflow".to_string())
        })
    }

    /// Gets the effective base fee with a 1-second cache (IGRA block time) to avoid one RPC per transaction.
    pub async fn get_effective_base_fee(&self, rpc_url: &str) -> Result<U256, AppError> {
        use std::time::{Duration, Instant};

        // Fast path: return cached value if still fresh.
        if let Some(cached) = {
            let guard = self.cache.read().await;
            guard.clone()
        } {
            if cached.fetched_at.elapsed() < Duration::from_secs(IGRA_BLOCK_TIME) {
                return Ok(cached.fee);
            }
        }

        // Cache is stale – fetch new value.
        let network_base_fee = self.fetch_network_base_fee(rpc_url).await?;
        let min_floor_wei = self.min_floor_wei()?;
        let effective_base_fee = std::cmp::max(network_base_fee, min_floor_wei);

        info!(
            "Effective base fee calculation: network_base_fee={} wei, min_floor_wei={} wei, effective_base_fee={} wei",
            network_base_fee, min_floor_wei, effective_base_fee
        );

        // Store in cache.
        {
            let mut guard = self.cache.write().await;
            *guard = Some(CachedFee {
                fee: effective_base_fee,
                fetched_at: Instant::now(),
            });
        }

        Ok(effective_base_fee)
    }

    /// Fetches the current base fee from the latest block.
    /// Assumes EIP-1559 is active and baseFeePerGas field exists.
    async fn fetch_network_base_fee(&self, rpc_url: &str) -> Result<U256, AppError> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "eth_getBlockByNumber",
            "params": ["latest", false],
            "id": 1
        });

        let response = el_caller::send_rpc_request(&request, rpc_url).await?;
        let block_response: JsonRpcResponse<Block> = serde_json::from_value(response)
            .map_err(|e| AppError::Internal(format!("Failed to parse block response: {}", e)))?;

        block_response.result.base_fee_per_gas.ok_or_else(|| {
            AppError::Internal(
                "Block missing baseFeePerGas field - EIP-1559 not active".to_string(),
            )
        })
    }

    /// Floors the gas price inside a JSON-RPC response Value (`eth_gasPrice`) in-place.
    /// If parsing fails or the price is already above the floor, the value is left unchanged.
    pub fn floor_gas_price_value(&self, response: &mut serde_json::Value) {
        // Fast exit if 'result' field is not a string
        let result = match response.get_mut("result") {
            Some(val) => val,
            None => return,
        };

        // Extract a copy of the current string value
        let current_hex = match result.as_str() {
            Some(s) => s,
            None => return,
        };

        // Trim prefix and parse.
        let trimmed = current_hex
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        if trimmed.is_empty() {
            return;
        }

        let price_wei = match U256::from_str_radix(trimmed, 16) {
            Ok(p) => p,
            Err(_) => return,
        };

        let min_floor_wei = match self.min_floor_wei() {
            Ok(v) => v,
            Err(_) => return,
        };

        if price_wei < min_floor_wei {
            info!(
                "Flooring gas price in Value. Original: {} Wei, New: {} Wei",
                price_wei, min_floor_wei
            );
            *result = serde_json::Value::String(format!("0x{:x}", min_floor_wei));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GasConfig;

    fn create_test_service(min_base_fee_gwei: u64) -> GasPriceService {
        GasPriceService::new(GasConfig { min_base_fee_gwei })
    }

    fn create_test_response_value(gas_price_hex: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": gas_price_hex
        })
    }

    fn get_price_from_value(v: &Value) -> U256 {
        let hex = v["result"].as_str().expect("result should be string");
        U256::from_str_radix(hex.trim_start_matches("0x"), 16).expect("Failed to parse hex")
    }

    #[test]
    fn test_gas_price_below_floor_is_floored() {
        let service = create_test_service(100);
        let price_wei = U256::from(50).saturating_mul(GWEI_TO_WEI); // 50 Gwei
        let price_hex = format!("0x{:x}", price_wei);
        let expected_wei = U256::from(100).saturating_mul(GWEI_TO_WEI);

        let mut response = create_test_response_value(&price_hex);
        service.floor_gas_price_value(&mut response);
        let final_price = get_price_from_value(&response);

        assert_eq!(final_price, expected_wei);
    }

    #[test]
    fn test_gas_price_above_floor_is_unchanged() {
        let service = create_test_service(100);
        let price_wei = U256::from(150).saturating_mul(GWEI_TO_WEI); // 150 Gwei
        let price_hex = format!("0x{:x}", price_wei);

        let mut response = create_test_response_value(&price_hex);
        service.floor_gas_price_value(&mut response);
        let final_price = get_price_from_value(&response);

        assert_eq!(final_price, price_wei);
    }

    #[test]
    fn test_gas_price_equal_to_floor_is_unchanged() {
        let service = create_test_service(100);
        let price_wei = U256::from(100).saturating_mul(GWEI_TO_WEI); // 100 Gwei
        let price_hex = format!("0x{:x}", price_wei);

        let mut response = create_test_response_value(&price_hex);
        service.floor_gas_price_value(&mut response);
        let final_price = get_price_from_value(&response);

        assert_eq!(final_price, price_wei);
    }

    #[test]
    fn test_invalid_hex_price_is_unchanged() {
        let service = create_test_service(100);
        let mut response = create_test_response_value("0xnot-a-hex-value");
        service.floor_gas_price_value(&mut response);
        assert_eq!(response["result"], "0xnot-a-hex-value");
    }

    #[tokio::test]
    async fn test_get_effective_base_fee_returns_higher_network_fee() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let server_url = server.uri();

        let service = create_test_service(100); // Floor is 100 Gwei
        let high_network_base_fee = U256::from(150).saturating_mul(GWEI_TO_WEI); // 150 Gwei

        let mock_block_response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "number": "0x1b4",
                "hash": "0x1234567890abcdef",
                "baseFeePerGas": format!("0x{:x}", high_network_base_fee)
            }
        });

        let expected_request = json!({
            "jsonrpc": "2.0",
            "method": "eth_getBlockByNumber",
            "params": ["latest", false],
            "id": 1
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_json(&expected_request))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_block_response))
            .expect(1)
            .mount(&server)
            .await;

        let result = service.get_effective_base_fee(&server_url).await;

        assert!(result.is_ok());
        assert_eq!(
            result.expect("Should get effective base fee"),
            high_network_base_fee
        );
    }

    #[tokio::test]
    async fn test_get_effective_base_fee_returns_floor_when_network_lower() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let server_url = server.uri();

        let service = create_test_service(100); // Floor is 100 Gwei
        let min_floor_wei = U256::from(100).saturating_mul(GWEI_TO_WEI);
        let low_network_base_fee = U256::from(50).saturating_mul(GWEI_TO_WEI); // 50 Gwei

        let mock_block_response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "number": "0x1b4",
                "hash": "0x1234567890abcdef",
                "baseFeePerGas": format!("0x{:x}", low_network_base_fee)
            }
        });

        let expected_request = json!({
            "jsonrpc": "2.0",
            "method": "eth_getBlockByNumber",
            "params": ["latest", false],
            "id": 1
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_json(&expected_request))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_block_response))
            .expect(1)
            .mount(&server)
            .await;

        let result = service.get_effective_base_fee(&server_url).await;

        assert!(result.is_ok());
        assert_eq!(
            result.expect("Should get effective base fee"),
            min_floor_wei
        );
    }

    #[tokio::test]
    async fn test_get_effective_base_fee_fails_without_base_fee_field() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let server_url = server.uri();
        let service = create_test_service(100);

        let mock_block_response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "number": "0x1b4",
                "hash": "0x1234567890abcdef"
                // No baseFeePerGas field - should fail
            }
        });

        let expected_request = json!({
            "jsonrpc": "2.0",
            "method": "eth_getBlockByNumber",
            "params": ["latest", false],
            "id": 1
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_json(&expected_request))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_block_response))
            .expect(1)
            .mount(&server)
            .await;

        let result = service.get_effective_base_fee(&server_url).await;

        assert!(result.is_err());
        assert!(result
            .expect_err("Should fail without baseFeePerGas")
            .to_string()
            .contains("EIP-1559 not active"));
    }
}
