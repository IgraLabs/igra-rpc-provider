use crate::config::GasConfig;
use crate::error::AppError;
use crate::types::rpc::JsonRpcResponse;
use serde_json::{from_slice, to_vec};
use tracing::{info, warn};

const GWEI_TO_WEI: u128 = 1_000_000_000;

/// A service to handle gas price logic, such as enforcing a minimum floor.
#[derive(Debug, Clone)]
pub struct GasPriceService {
    config: GasConfig,
}

impl GasPriceService {
    /// Creates a new `GasPriceService`.
    pub fn new(config: GasConfig) -> Self {
        Self { config }
    }

    /// Takes a raw JSON-RPC response body and enforces the minimum gas price floor.
    /// This method is designed to work with the `eth_gasPrice` RPC call response.
    ///
    /// If the method is not `eth_gasPrice` or parsing fails, it returns the original body.
    pub fn floor_gas_price_response(&self, body: &[u8]) -> Result<Vec<u8>, AppError> {
        // Attempt to deserialize the body into a generic JSON-RPC response with a hex string result.
        let mut response: JsonRpcResponse<String> = match from_slice(body) {
            Ok(response) => response,
            Err(e) => {
                warn!("Failed to deserialize JSON-RPC response: {}", e);
                return Ok(body.to_vec());
            }
        };

        // Parse the hex string result into a u128 value.
        let reth_price_wei =
            match u128::from_str_radix(response.result.trim_start_matches("0x"), 16) {
                Ok(price) => price,
                Err(e) => {
                    warn!("Failed to parse gas price: {}", e);
                    return Ok(body.to_vec());
                }
            };

        // Calculate the minimum floor price in Wei using safe arithmetic.
        let min_floor_wei = u128::from(self.config.min_base_fee_gwei).saturating_mul(GWEI_TO_WEI);

        // If reth's price is lower than the floor, replace it.
        if reth_price_wei < min_floor_wei {
            info!(
                "Flooring gas price. Original: {} Wei, New: {} Wei",
                reth_price_wei, min_floor_wei
            );
            response.result = format!("0x{:x}", min_floor_wei);
        }

        // Serialize the (potentially modified) response back into bytes.
        to_vec(&response).map_err(|e| AppError::Internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GasConfig;
    use serde_json::json;

    fn create_test_service(min_base_fee_gwei: u64) -> GasPriceService {
        GasPriceService::new(GasConfig { min_base_fee_gwei })
    }

    fn create_test_response_body(gas_price_hex: &str) -> Vec<u8> {
        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": gas_price_hex
        });
        serde_json::to_vec(&response).expect("Should be able to create test response body")
    }

    fn get_price_from_body(body: &[u8]) -> u128 {
        let response: JsonRpcResponse<String> =
            serde_json::from_slice(body).expect("Should be able to parse response body");
        u128::from_str_radix(response.result.trim_start_matches("0x"), 16)
            .expect("Result should be a valid hex number")
    }

    #[test]
    fn test_gas_price_below_floor_is_floored() {
        // Arrange
        let service = create_test_service(100); // Floor is 100 Gwei
        let price_50_gwei_wei = 50u128.saturating_mul(GWEI_TO_WEI);
        let price_50_gwei_hex = format!("0x{:x}", price_50_gwei_wei);
        let body = create_test_response_body(&price_50_gwei_hex);
        let expected_floor_price_wei = 100u128.saturating_mul(GWEI_TO_WEI);

        // Act
        let result_body = service
            .floor_gas_price_response(&body)
            .expect("Flooring should not fail");
        let final_price = get_price_from_body(&result_body);

        // Assert
        assert_eq!(final_price, expected_floor_price_wei);
    }

    #[test]
    fn test_gas_price_above_floor_is_unchanged() {
        // Arrange
        let service = create_test_service(100); // Floor is 100 Gwei
        let price_150_gwei_wei = 150u128.saturating_mul(GWEI_TO_WEI);
        let price_150_gwei_hex = format!("0x{:x}", price_150_gwei_wei);
        let body = create_test_response_body(&price_150_gwei_hex);

        // Act
        let result_body = service
            .floor_gas_price_response(&body)
            .expect("Flooring should not fail");
        let final_price = get_price_from_body(&result_body);

        // Assert
        assert_eq!(final_price, price_150_gwei_wei);
    }

    #[test]
    fn test_gas_price_equal_to_floor_is_unchanged() {
        // Arrange
        let service = create_test_service(100); // Floor is 100 Gwei
        let price_100_gwei_wei = 100u128.saturating_mul(GWEI_TO_WEI);
        let price_100_gwei_hex = format!("0x{:x}", price_100_gwei_wei);
        let body = create_test_response_body(&price_100_gwei_hex);

        // Act
        let result_body = service
            .floor_gas_price_response(&body)
            .expect("Flooring should not fail");
        let final_price = get_price_from_body(&result_body);

        // Assert
        assert_eq!(final_price, price_100_gwei_wei);
    }

    #[test]
    fn test_invalid_json_returns_original_body() {
        // Arrange
        let service = create_test_service(100);
        let invalid_body = b"this is not json";

        // Act
        let result_body = service
            .floor_gas_price_response(invalid_body)
            .expect("Flooring should not fail on invalid json");

        // Assert
        assert_eq!(result_body, invalid_body);
    }

    #[test]
    fn test_invalid_hex_price_returns_original_body() {
        // Arrange
        let service = create_test_service(100);
        let body = create_test_response_body("0xnot-a-hex-value");

        // Act
        let result_body = service
            .floor_gas_price_response(&body)
            .expect("Flooring should not fail on invalid hex");

        // Assert
        assert_eq!(result_body, body);
    }
}
