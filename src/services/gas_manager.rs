use crate::clients::el_caller;
use crate::config::GasConfig;
use crate::error::AppError;
use crate::types::rpc::Block;
use alloy::primitives::U256;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

const GWEI_TO_WEI: u128 = 1_000_000_000;
const IGRA_BLOCK_TIME: u64 = 1;

/// Service responsible for all gas price management and validation
#[derive(Debug, Clone)]
pub struct GasManager {
    config: GasConfig,
    cache: Arc<RwLock<Option<CachedFee>>>,
}

/// Holds a cached fee value and the instant it was obtained
#[derive(Debug, Clone)]
struct CachedFee {
    fee: U256,
    fetched_at: Instant,
}

impl GasManager {
    /// Creates a new GasManager with the given configuration
    pub fn new(config: GasConfig) -> Self {
        Self {
            config,
            cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Calculate the configured minimum floor in Wei using checked arithmetic
    fn min_floor_wei(&self) -> Result<U256, AppError> {
        let gwei = U256::from(self.config.min_protocol_fee_per_gas_gwei);
        let gwei_to_wei = U256::from(GWEI_TO_WEI);
        gwei.checked_mul(gwei_to_wei).ok_or_else(|| {
            AppError::Internal("min_protocol_fee_per_gas_gwei multiplication overflow".to_string())
        })
    }

    /// Get the effective base fee with caching to avoid frequent RPC calls
    pub async fn get_effective_base_fee(&self, rpc_url: &str) -> Result<U256, AppError> {
        // Check if we have a valid cached value
        let cached_fee = {
            let cache_guard = self.cache.read().await;
            cache_guard.clone()
        };

        if let Some(cached) = cached_fee {
            let cache_age = cached.fetched_at.elapsed();
            let cache_duration = Duration::from_secs(IGRA_BLOCK_TIME);

            if cache_age <= cache_duration {
                debug!(
                    "GAS_MANAGER: Using cached effective base fee: {} wei (age: {:?})",
                    cached.fee, cache_age
                );
                return Ok(cached.fee);
            }
        }

        // Cache miss or expired, fetch fresh data
        let fresh_fee = self.fetch_fresh_base_fee(rpc_url).await?;

        // Update cache
        {
            let mut cache_guard = self.cache.write().await;
            *cache_guard = Some(CachedFee {
                fee: fresh_fee,
                fetched_at: Instant::now(),
            });
        }

        info!(
            "GAS_MANAGER: Updated effective base fee cache: {} wei",
            fresh_fee
        );

        Ok(fresh_fee)
    }

    /// Fetch fresh base fee from the EL client
    async fn fetch_fresh_base_fee(&self, rpc_url: &str) -> Result<U256, AppError> {
        debug!(
            "GAS_MANAGER: Fetching fresh base fee from EL at {}",
            rpc_url
        );

        // Get the latest block to extract base fee
        let latest_block_request = json!({
            "jsonrpc": "2.0",
            "method": "eth_getBlockByNumber",
            "params": ["latest", false],
            "id": 1
        });

        let response = el_caller::send_rpc_request(&latest_block_request, rpc_url)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to fetch latest block: {e}")))?;

        let block: Block = serde_json::from_value(
            response
                .get("result")
                .ok_or_else(|| AppError::Internal("Missing result in block response".to_string()))?
                .clone(),
        )
        .map_err(|e| AppError::Internal(format!("Failed to parse block: {e}")))?;

        let base_fee_per_gas = block
            .base_fee_per_gas
            .ok_or_else(|| AppError::Internal("Block missing baseFeePerGas".to_string()))?;

        let min_floor = self.min_floor_wei()?;
        let effective_base_fee = std::cmp::max(base_fee_per_gas, min_floor);

        debug!(
            "GAS_MANAGER: Block base fee: {} wei, configured floor: {} wei, effective: {} wei",
            base_fee_per_gas, min_floor, effective_base_fee
        );

        Ok(effective_base_fee)
    }

    /// Validate EIP-1559 transaction fees against effective base fee
    pub fn validate_eip1559_fees(
        &self,
        max_fee_per_gas: U256,
        max_priority_fee_per_gas: U256,
        effective_base_fee: U256,
    ) -> Result<bool, AppError> {
        // Validate that max_priority_fee_per_gas <= max_fee_per_gas
        if max_priority_fee_per_gas > max_fee_per_gas {
            warn!(
                "GAS_MANAGER: EIP-1559 validation failed - priority fee ({}) exceeds max fee ({})",
                max_priority_fee_per_gas, max_fee_per_gas
            );
            return Ok(false);
        }

        // Validate that max_fee_per_gas >= effective_base_fee
        if max_fee_per_gas < effective_base_fee {
            warn!(
                "GAS_MANAGER: EIP-1559 validation failed - max fee ({}) below effective base fee ({})",
                max_fee_per_gas, effective_base_fee
            );
            return Ok(false);
        }

        // Validate that the effective total fee is reasonable
        let total_fee = effective_base_fee
            .checked_add(max_priority_fee_per_gas)
            .ok_or_else(|| AppError::Internal("Fee calculation overflow".to_string()))?;

        if total_fee > max_fee_per_gas {
            // This should not happen if the previous checks passed, but let's be safe
            warn!(
                "GAS_MANAGER: EIP-1559 validation failed - calculated total fee ({}) exceeds max fee ({})",
                total_fee, max_fee_per_gas
            );
            return Ok(false);
        }

        debug!(
            "GAS_MANAGER: EIP-1559 fees validated - max_fee: {}, priority_fee: {}, base_fee: {}, total: {}",
            max_fee_per_gas, max_priority_fee_per_gas, effective_base_fee, total_fee
        );

        Ok(true)
    }

    /// Validate legacy transaction gas price against effective base fee
    pub fn validate_legacy_gas_price(&self, gas_price: U256, effective_base_fee: U256) -> bool {
        let is_valid = gas_price >= effective_base_fee;

        if is_valid {
            debug!(
                "GAS_MANAGER: Legacy gas price validated - gas_price: {}, base_fee: {}",
                gas_price, effective_base_fee
            );
        } else {
            warn!(
                "GAS_MANAGER: Legacy gas price validation failed - gas_price: {}, base_fee: {}",
                gas_price, effective_base_fee
            );
        }

        is_valid
    }

    /// Apply gas price floor to an eth_gasPrice response
    pub fn floor_gas_price_value(&self, response: &mut Value) {
        if let Some(result) = response.get_mut("result") {
            if let Some(gas_price_str) = result.as_str().map(|s| s.to_string()) {
                match self.apply_gas_price_floor(&gas_price_str) {
                    Ok(floored_price) => {
                        *result = json!(format!("0x{:x}", floored_price));
                        info!(
                            "GAS_MANAGER: Applied gas price floor - original: {}, floored: 0x{:x}",
                            gas_price_str, floored_price
                        );
                    }
                    Err(e) => {
                        error!(
                            "GAS_MANAGER: Failed to apply gas price floor to '{}': {}",
                            gas_price_str, e
                        );
                    }
                }
            }
        }
    }

    /// Apply the configured gas price floor to a gas price value
    fn apply_gas_price_floor(&self, gas_price_hex: &str) -> Result<U256, AppError> {
        // Parse the hex gas price
        let gas_price = if gas_price_hex.starts_with("0x") || gas_price_hex.starts_with("0X") {
            U256::from_str_radix(&gas_price_hex[2..], 16)
        } else {
            U256::from_str_radix(gas_price_hex, 16)
        }
        .map_err(|e| AppError::Internal(format!("Invalid gas price hex: {e}")))?;

        let min_floor = self.min_floor_wei()?;
        let floored_price = std::cmp::max(gas_price, min_floor);

        Ok(floored_price)
    }

    /// Get current gas pricing configuration
    pub fn get_config(&self) -> &GasConfig {
        &self.config
    }

    /// Update gas pricing configuration
    pub fn update_config(&mut self, new_config: GasConfig) {
        info!(
            "GAS_MANAGER: Updating configuration - old min_protocol_fee_per_gas: {} gwei, new: {} gwei",
            self.config.min_protocol_fee_per_gas_gwei, new_config.min_protocol_fee_per_gas_gwei
        );
        self.config = new_config;

        // Clear cache to force refresh with new configuration
        let cache = self.cache.clone();
        tokio::spawn(async move {
            let mut cache_guard = cache.write().await;
            *cache_guard = None;
        });
    }

    /// Clear the fee cache (useful for testing or forced refresh)
    pub async fn clear_cache(&self) {
        let mut cache_guard = self.cache.write().await;
        *cache_guard = None;
        info!("GAS_MANAGER: Fee cache cleared");
    }

    /// Get cache statistics for monitoring
    pub async fn get_cache_stats(&self) -> Option<(U256, Duration)> {
        let cache_guard = self.cache.read().await;
        cache_guard
            .as_ref()
            .map(|cached| (cached.fee, cached.fetched_at.elapsed()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GasConfig;

    fn create_test_gas_manager() -> GasManager {
        let config = GasConfig {
            min_protocol_fee_per_gas_gwei: 10, // 10 gwei minimum
        };
        GasManager::new(config)
    }

    #[test]
    fn test_min_floor_wei_calculation() {
        let gas_manager = create_test_gas_manager();
        let min_floor = gas_manager
            .min_floor_wei()
            .expect("min_floor_wei should not fail with valid config");
        let expected = U256::from(10) * U256::from(GWEI_TO_WEI); // 10 gwei in wei
        assert_eq!(min_floor, expected);
    }

    #[test]
    fn test_validate_eip1559_fees_valid() {
        let gas_manager = create_test_gas_manager();
        let max_fee = U256::from(20_000_000_000u64); // 20 gwei
        let priority_fee = U256::from(2_000_000_000u64); // 2 gwei
        let base_fee = U256::from(15_000_000_000u64); // 15 gwei

        let result = gas_manager.validate_eip1559_fees(max_fee, priority_fee, base_fee);
        assert!(result.expect("validation should not fail with valid inputs"));
    }

    #[test]
    fn test_validate_eip1559_fees_invalid_priority_too_high() {
        let gas_manager = create_test_gas_manager();
        let max_fee = U256::from(20_000_000_000u64); // 20 gwei
        let priority_fee = U256::from(25_000_000_000u64); // 25 gwei (too high)
        let base_fee = U256::from(15_000_000_000u64); // 15 gwei

        let result = gas_manager.validate_eip1559_fees(max_fee, priority_fee, base_fee);
        assert!(!result.expect("validation should not fail"));
    }

    #[test]
    fn test_validate_eip1559_fees_invalid_max_too_low() {
        let gas_manager = create_test_gas_manager();
        let max_fee = U256::from(10_000_000_000u64); // 10 gwei (too low)
        let priority_fee = U256::from(2_000_000_000u64); // 2 gwei
        let base_fee = U256::from(15_000_000_000u64); // 15 gwei

        let result = gas_manager.validate_eip1559_fees(max_fee, priority_fee, base_fee);
        assert!(!result.expect("validation should not fail"));
    }

    #[test]
    fn test_validate_legacy_gas_price() {
        let gas_manager = create_test_gas_manager();
        let gas_price = U256::from(20_000_000_000u64); // 20 gwei
        let base_fee = U256::from(15_000_000_000u64); // 15 gwei

        assert!(gas_manager.validate_legacy_gas_price(gas_price, base_fee));

        // Test with gas price too low
        let low_gas_price = U256::from(10_000_000_000u64); // 10 gwei
        assert!(!gas_manager.validate_legacy_gas_price(low_gas_price, base_fee));
    }

    #[test]
    fn test_apply_gas_price_floor() {
        let gas_manager = create_test_gas_manager();

        // Test with gas price above floor
        let high_price = "0x4a817c800"; // 20 gwei in hex
        let result = gas_manager
            .apply_gas_price_floor(high_price)
            .expect("apply_gas_price_floor should succeed with valid input");
        assert_eq!(result, U256::from(20_000_000_000u64));

        // Test with gas price below floor (should be raised to floor)
        let low_price = "0x1dcd6500"; // 8 gwei in hex
        let result = gas_manager
            .apply_gas_price_floor(low_price)
            .expect("apply_gas_price_floor should succeed with valid input");
        let expected_floor = U256::from(10) * U256::from(GWEI_TO_WEI); // 10 gwei floor
        assert_eq!(result, expected_floor);
    }
}
