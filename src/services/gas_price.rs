use crate::clients::el_caller;
use crate::config::GasConfig;
use crate::error::AppError;
use crate::types::rpc::{Block, JsonRpcResponse};
use alloy::primitives::U256;
use serde_json::json;
use serde_json::Value;
use tracing::info;

const GWEI_TO_WEI: u128 = 1_000_000_000;
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
        let gwei = U256::from(self.config.min_protocol_fee_per_gas_gwei);
        let gwei_to_wei = U256::from(GWEI_TO_WEI);
        gwei.checked_mul(gwei_to_wei).ok_or_else(|| {
            AppError::Internal("min_protocol_fee_per_gas_gwei multiplication overflow".to_string())
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
            .map_err(|e| AppError::Internal(format!("Failed to parse block response: {e}")))?;

        block_response.result.base_fee_per_gas.ok_or_else(|| {
            AppError::Internal(
                "Block missing baseFeePerGas field - EIP-1559 not active".to_string(),
            )
        })
    }

    /// Floors a single-quantity JSON-RPC response in-place (`eth_gasPrice` /
    /// `eth_maxPriorityFeePerGas`). The `result` field is expected to be a `0x`-prefixed hex
    /// quantity; if parsing fails or the value is already at/above the floor it is left unchanged.
    /// Flooring `eth_maxPriorityFeePerGas` is what makes default EIP-1559 wallets
    /// (viem/Rabby/MetaMask) select a tip that satisfies the protocol minimum.
    pub fn floor_gas_price_value(&self, response: &mut serde_json::Value) {
        let min_floor_wei = match self.min_floor_wei() {
            Ok(v) => v,
            Err(_) => return,
        };

        if let Some(result) = response.get_mut("result") {
            if floor_hex_quantity_in_place(result, min_floor_wei) {
                info!(
                    "Floored single-quantity fee response up to protocol minimum {} wei",
                    min_floor_wei
                );
            }
        }
    }

    /// Floors the `reward` (priority-fee) percentiles inside an `eth_feeHistory` response in-place.
    ///
    /// Only `result.reward[][]` is floored — `baseFeePerGas`, `gasUsedRatio`, and `oldestBlock` are
    /// left untouched (leaving `baseFeePerGas` alone avoids inflating the wallet's `maxFee`/balance
    /// reservation). The tip is floored to the *static* configured minimum
    /// (`min_protocol_fee_per_gas_gwei`). Note the transaction validator enforces a *dynamic*
    /// threshold — `max_priority_fee_per_gas >= max(network_base_fee, configured_minimum)` — so this
    /// static floor fully matches the validator only while `network_base_fee <= configured_minimum`
    /// (the normal case on a near-zero-base-fee chain). If the network base fee could exceed the
    /// floor, the oracle would need to advertise `get_effective_base_fee` instead.
    /// Missing or malformed fields, and EL error responses (no `result`), are left unchanged.
    pub fn floor_fee_history_value(&self, response: &mut serde_json::Value) {
        let min_floor_wei = match self.min_floor_wei() {
            Ok(v) => v,
            Err(_) => return,
        };

        let result = match response.get_mut("result") {
            Some(result) => result,
            None => return,
        };

        // `reward` is a per-block array of per-percentile hex quantities; present only when the
        // request supplied reward percentiles.
        let rewards = match result.get_mut("reward").and_then(Value::as_array_mut) {
            Some(rewards) => rewards,
            None => return,
        };

        let mut floored: usize = 0;
        for per_block in rewards.iter_mut() {
            if let Some(percentiles) = per_block.as_array_mut() {
                for entry in percentiles.iter_mut() {
                    if floor_hex_quantity_in_place(entry, min_floor_wei) {
                        floored = floored.saturating_add(1);
                    }
                }
            }
        }

        if floored > 0 {
            info!(
                "Floored {} eth_feeHistory reward value(s) up to protocol minimum {} wei",
                floored, min_floor_wei
            );
        }
    }
}

/// Floors a single hex-quantity JSON value (e.g. `"0x3b9aca00"`) up to `min_floor_wei`, in place.
///
/// Leaves the value untouched when it is not a parseable `0x`-prefixed hex string, or is already
/// at/above the floor. Returns `true` only when the value was raised to the floor.
fn floor_hex_quantity_in_place(value: &mut Value, min_floor_wei: U256) -> bool {
    let current_hex = match value.as_str() {
        Some(s) => s,
        None => return false,
    };

    let trimmed = current_hex
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    if trimmed.is_empty() {
        return false;
    }

    let price_wei = match U256::from_str_radix(trimmed, 16) {
        Ok(p) => p,
        Err(_) => return false,
    };

    if price_wei < min_floor_wei {
        *value = Value::String(format!("0x{min_floor_wei:x}"));
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GasConfig;

    fn create_test_service(min_protocol_fee_per_gas_gwei: u64) -> GasPriceService {
        GasPriceService::new(GasConfig {
            min_protocol_fee_per_gas_gwei,
        })
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

    fn gwei_to_wei(gwei: u64) -> U256 {
        U256::from(gwei).saturating_mul(U256::from(GWEI_TO_WEI))
    }

    #[test]
    fn test_gas_price_below_floor_is_floored() {
        let service = create_test_service(100);
        let price_wei = gwei_to_wei(50); // 50 Gwei
        let price_hex = format!("0x{price_wei:x}");
        let expected_wei = gwei_to_wei(100);

        let mut response = create_test_response_value(&price_hex);
        service.floor_gas_price_value(&mut response);
        let final_price = get_price_from_value(&response);

        assert_eq!(final_price, expected_wei);
    }

    #[test]
    fn test_gas_price_above_floor_is_unchanged() {
        let service = create_test_service(100);
        let price_wei = gwei_to_wei(150); // 150 Gwei
        let price_hex = format!("0x{price_wei:x}");

        let mut response = create_test_response_value(&price_hex);
        service.floor_gas_price_value(&mut response);
        let final_price = get_price_from_value(&response);

        assert_eq!(final_price, price_wei);
    }

    #[test]
    fn test_gas_price_equal_to_floor_is_unchanged() {
        let service = create_test_service(100);
        let price_wei = gwei_to_wei(100); // 100 Gwei
        let price_hex = format!("0x{price_wei:x}");

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
        let high_network_base_fee = gwei_to_wei(150); // 150 Gwei

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
        let min_floor_wei = gwei_to_wei(100);
        let low_network_base_fee = gwei_to_wei(50); // 50 Gwei

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

    // --- eth_maxPriorityFeePerGas (same single-quantity shape as eth_gasPrice) ---

    #[test]
    fn test_max_priority_fee_below_floor_is_floored() {
        let service = create_test_service(100);
        let one_gwei = format!("0x{:x}", gwei_to_wei(1));
        let mut response = create_test_response_value(&one_gwei);
        service.floor_gas_price_value(&mut response);
        assert_eq!(get_price_from_value(&response), gwei_to_wei(100));
    }

    // --- eth_feeHistory reward flooring ---

    fn fee_history_response(base_fees: Value, reward: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "oldestBlock": "0x1",
                "baseFeePerGas": base_fees,
                "gasUsedRatio": [0.5, 0.5],
                "reward": reward
            }
        })
    }

    fn reward_entry(v: &Value, block: usize, pct: usize) -> U256 {
        let hex = v["result"]["reward"][block][pct]
            .as_str()
            .expect("reward entry should be a string");
        U256::from_str_radix(hex.trim_start_matches("0x"), 16).expect("Failed to parse hex")
    }

    #[test]
    fn test_fee_history_reward_below_floor_is_floored() {
        let service = create_test_service(100);
        let one_gwei = format!("0x{:x}", gwei_to_wei(1));
        let mut response = fee_history_response(
            json!(["0x1", "0x1"]),
            json!([["0x0", one_gwei], ["0x0", "0x0"]]),
        );
        service.floor_fee_history_value(&mut response);
        assert_eq!(reward_entry(&response, 0, 0), gwei_to_wei(100));
        assert_eq!(reward_entry(&response, 0, 1), gwei_to_wei(100));
        assert_eq!(reward_entry(&response, 1, 0), gwei_to_wei(100));
    }

    #[test]
    fn test_fee_history_does_not_touch_base_fee() {
        let service = create_test_service(100);
        let mut response = fee_history_response(json!(["0x1", "0x1"]), json!([["0x0"]]));
        service.floor_fee_history_value(&mut response);
        // baseFeePerGas must be left untouched (avoids inflating wallet maxFee/balance).
        assert_eq!(response["result"]["baseFeePerGas"][0], "0x1");
        assert_eq!(response["result"]["baseFeePerGas"][1], "0x1");
        // reward still floored.
        assert_eq!(reward_entry(&response, 0, 0), gwei_to_wei(100));
    }

    #[test]
    fn test_fee_history_reward_above_floor_unchanged() {
        let service = create_test_service(100);
        let high = format!("0x{:x}", gwei_to_wei(150));
        let mut response = fee_history_response(json!(["0x1"]), json!([[high]]));
        service.floor_fee_history_value(&mut response);
        assert_eq!(reward_entry(&response, 0, 0), gwei_to_wei(150));
    }

    #[test]
    fn test_fee_history_missing_reward_is_noop() {
        let service = create_test_service(100);
        let mut response = json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "oldestBlock": "0x1", "baseFeePerGas": ["0x1"], "gasUsedRatio": [0.5] }
        });
        let before = response.clone();
        service.floor_fee_history_value(&mut response);
        assert_eq!(response, before);
    }

    #[test]
    fn test_fee_history_malformed_entries_unchanged() {
        let service = create_test_service(100);
        let mut response = fee_history_response(json!(["0x1"]), json!([[Value::Null, "0xnothex"]]));
        service.floor_fee_history_value(&mut response);
        assert_eq!(response["result"]["reward"][0][0], Value::Null);
        assert_eq!(response["result"]["reward"][0][1], "0xnothex");
    }

    #[test]
    fn test_fee_history_error_response_unchanged() {
        let service = create_test_service(100);
        let mut response = json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32000, "message": "boom" }
        });
        let before = response.clone();
        service.floor_fee_history_value(&mut response);
        assert_eq!(response, before);
    }
}
