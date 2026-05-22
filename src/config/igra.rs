//! IGRA lane configuration.
//!
//! Exposes a single canonical input form for the lane id, matching the
//! kaswallet daemon's `--subnetwork-id` exactly: a 4-byte namespace as 8
//! lowercase hex chars (no `0x`, e.g. `97b10000`). The deserializer
//! zero-pads per KIP-21 to the full 20-byte `SubnetworkId`
//! (`namespace ++ [0; 16]`) and stores `[u8; 20]` for direct byte-equality
//! with `tx.subnetwork_id` on the wire.

use crate::error::AppError;
use serde::{Deserialize, Deserializer};

/// Length of the 4-byte SubnetworkId namespace in bytes.
const LANE_NAMESPACE_BYTE_LEN: usize = 4;
/// Total SubnetworkId length in bytes (per KIP-21).
const SUBNETWORK_ID_SIZE: usize = 20;
/// Length of the 4-byte SubnetworkId namespace in hex characters.
const LANE_NAMESPACE_HEX_LEN: usize = LANE_NAMESPACE_BYTE_LEN * 2;
/// 20-byte SubnetworkId for the native lane (all zero), which is not a
/// valid IGRA lane and is rejected.
const NATIVE_LANE_ID: [u8; SUBNETWORK_ID_SIZE] = [0u8; SUBNETWORK_ID_SIZE];

/// IGRA lane configuration.
///
/// Holds the full 20-byte SubnetworkId. The deserializer and the
/// [`IgraConfig::from_namespace`] constructor both accept only the
/// 4-byte namespace and zero-pad per KIP-21. The field is `pub(crate)`
/// so external code can only construct values through one of those
/// shape-preserving paths.
#[derive(Debug, Clone, Deserialize)]
pub struct IgraConfig {
    /// Full 20-byte SubnetworkId (namespace + 16 zero bytes per KIP-21).
    #[serde(deserialize_with = "deserialize_lane_id")]
    pub(crate) lane_id: [u8; SUBNETWORK_ID_SIZE],
}

impl IgraConfig {
    /// Build an `IgraConfig` from a 4-byte namespace, zero-padding to
    /// the full 20-byte SubnetworkId per KIP-21.
    ///
    /// Does not enforce the NATIVE / reserved-system rejection — that
    /// runs in [`IgraConfig::validate`]. Use this constructor in tests
    /// or wherever programmatic construction is needed; production code
    /// should let the deserializer + `validate()` enforce the full
    /// invariant set.
    pub fn from_namespace(namespace: [u8; LANE_NAMESPACE_BYTE_LEN]) -> Self {
        let mut lane_id = [0u8; SUBNETWORK_ID_SIZE];
        lane_id[..LANE_NAMESPACE_BYTE_LEN].copy_from_slice(&namespace);
        Self { lane_id }
    }

    /// Read-only accessor for the lane id. Returns a copy of the
    /// 20-byte SubnetworkId; the caller cannot mutate the configured
    /// lane through this handle.
    pub fn lane_id(&self) -> [u8; SUBNETWORK_ID_SIZE] {
        self.lane_id
    }

    /// Validates the IGRA configuration.
    ///
    /// Enforces three invariants on `lane_id`:
    /// 1. Not the NATIVE lane (`[0; 20]`).
    /// 2. KIP-21 zero-padded shape: bytes 4..20 are all zero. The
    ///    deserializer and `from_namespace` guarantee this on
    ///    construction; this check defends against future code paths
    ///    that mutate the field directly.
    /// 3. Not the reserved-system shape `[x, 0×19]` for `x != 0` — this
    ///    catches the COINBASE (`01000000`) and REGISTRY (`02000000`)
    ///    builtin subnetworks plus any 4-byte namespace whose bytes
    ///    1..4 are all zero. The kaswallet daemon's parser rejects
    ///    these on its side; enforcing here gives operators a clearer
    ///    startup error than a per-transaction daemon rejection.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.lane_id == NATIVE_LANE_ID {
            return Err(AppError::ConfigError(
                "igra.lane_id must not be the NATIVE lane (00000000); \
                 set IGRA_LANE_ID to a non-native 4-byte namespace such as 97b10000"
                    .to_string(),
            ));
        }
        if self.lane_id[LANE_NAMESPACE_BYTE_LEN..]
            .iter()
            .any(|&b| b != 0)
        {
            return Err(AppError::ConfigError(format!(
                "igra.lane_id must be zero-padded per KIP-21 \
                 (bytes {LANE_NAMESPACE_BYTE_LEN}..{SUBNETWORK_ID_SIZE} all zero), got 0x{}",
                hex::encode(self.lane_id),
            )));
        }
        if self.lane_id[1..LANE_NAMESPACE_BYTE_LEN]
            .iter()
            .all(|&b| b == 0)
        {
            // bytes 1..4 all zero AND first byte non-zero (NATIVE was caught above)
            // → reserved-system shape [x, 0×19], rejected by consensus
            return Err(AppError::ConfigError(format!(
                "igra.lane_id 0x{:02x}000000 collides with KIP-21 reserved-system shape \
                 [x, 0×19] (covers built-in subnetworks COINBASE/REGISTRY); \
                 set a 4-byte namespace whose bytes 1..4 are not all zero, \
                 such as 97b10000",
                self.lane_id[0],
            )));
        }
        Ok(())
    }
}

/// Strict deserializer for the lane id.
///
/// Accepts only an 8-character lowercase hex string. Rejects `0x` prefix,
/// uppercase, non-hex, wrong length, and the NATIVE namespace
/// (`00000000`). Zero-pads to 20 bytes per KIP-21.
fn deserialize_lane_id<'de, D>(deserializer: D) -> Result<[u8; SUBNETWORK_ID_SIZE], D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct LaneIdVisitor;

    impl Visitor<'_> for LaneIdVisitor {
        type Value = [u8; SUBNETWORK_ID_SIZE];

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str(
                "a 4-byte SubnetworkId namespace as exactly 8 lowercase hex chars, \
                 no 0x prefix (e.g. \"97b10000\")",
            )
        }

        fn visit_str<E>(self, value: &str) -> Result<[u8; SUBNETWORK_ID_SIZE], E>
        where
            E: de::Error,
        {
            if value.starts_with("0x") || value.starts_with("0X") {
                return Err(de::Error::custom(
                    "igra.lane_id must not have a 0x prefix; expected 8 lowercase hex chars \
                     (e.g. 97b10000)",
                ));
            }
            if value.len() != LANE_NAMESPACE_HEX_LEN {
                return Err(de::Error::custom(format!(
                    "igra.lane_id must be exactly {LANE_NAMESPACE_HEX_LEN} lowercase hex chars \
                     (the 4-byte SubnetworkId namespace, e.g. 97b10000), got {} chars",
                    value.len(),
                )));
            }
            if value.chars().any(|c| c.is_ascii_uppercase()) {
                return Err(de::Error::custom(
                    "igra.lane_id must be lowercase hex (e.g. 97b10000)",
                ));
            }
            let mut namespace = [0u8; LANE_NAMESPACE_BYTE_LEN];
            hex::decode_to_slice(value, &mut namespace).map_err(|e| {
                de::Error::custom(format!(
                    "igra.lane_id is not valid lowercase hex (e.g. 97b10000): {e}"
                ))
            })?;
            if namespace == [0u8; LANE_NAMESPACE_BYTE_LEN] {
                return Err(de::Error::custom(
                    "igra.lane_id must not be the NATIVE lane (00000000); \
                     set a non-native 4-byte namespace such as 97b10000",
                ));
            }
            if namespace[1..].iter().all(|&b| b == 0) {
                return Err(de::Error::custom(format!(
                    "igra.lane_id {value} collides with KIP-21 reserved-system shape \
                     [x, 0×19] (covers built-in subnetworks COINBASE/REGISTRY); \
                     set a 4-byte namespace whose bytes 1..4 are not all zero, \
                     such as 97b10000"
                )));
            }
            let mut padded = [0u8; SUBNETWORK_ID_SIZE];
            padded[..LANE_NAMESPACE_BYTE_LEN].copy_from_slice(&namespace);
            Ok(padded)
        }
    }

    deserializer.deserialize_str(LaneIdVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_src: &str) -> Result<IgraConfig, toml::de::Error> {
        toml::from_str(toml_src)
    }

    #[test]
    fn accepts_canonical_8_hex_namespace() {
        let cfg = parse(r#"lane_id = "97b10000""#).expect("should accept canonical 8 hex chars");
        let mut expected = [0u8; 20];
        expected[0] = 0x97;
        expected[1] = 0xb1;
        // bytes 2..20 stay zero
        assert_eq!(cfg.lane_id, expected);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn zero_pads_to_full_20_bytes() {
        let cfg = parse(r#"lane_id = "deadbeef""#).expect("should accept 8 hex chars");
        let mut expected = [0u8; 20];
        expected[0] = 0xde;
        expected[1] = 0xad;
        expected[2] = 0xbe;
        expected[3] = 0xef;
        assert_eq!(cfg.lane_id, expected);
        // last 16 bytes are zero per KIP-21
        assert!(cfg.lane_id[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn rejects_native_lane() {
        let err = parse(r#"lane_id = "00000000""#)
            .expect_err("NATIVE lane (00000000) must be rejected")
            .to_string();
        assert!(err.contains("NATIVE"), "error was: {err}");
    }

    #[test]
    fn rejects_0x_prefix() {
        let err = parse(r#"lane_id = "0x97b10000""#)
            .expect_err("0x prefix must be rejected")
            .to_string();
        assert!(err.contains("0x prefix"), "error was: {err}");
    }

    #[test]
    fn rejects_wrong_length_too_short() {
        let err = parse(r#"lane_id = "97b1""#)
            .expect_err("short lane id must be rejected")
            .to_string();
        assert!(err.contains("8 lowercase hex"), "error was: {err}");
    }

    #[test]
    fn rejects_wrong_length_too_long_40_chars() {
        let err = parse(r#"lane_id = "97b10000000000000000000000000000000000000""#)
            .expect_err("40-char lane id must be rejected (must be 8 chars)")
            .to_string();
        assert!(err.contains("8 lowercase hex"), "error was: {err}");
    }

    #[test]
    fn rejects_uppercase_hex() {
        let err = parse(r#"lane_id = "97B10000""#)
            .expect_err("uppercase hex must be rejected")
            .to_string();
        assert!(err.contains("lowercase"), "error was: {err}");
    }

    #[test]
    fn rejects_non_hex() {
        let err = parse(r#"lane_id = "97g10000""#)
            .expect_err("non-hex chars must be rejected")
            .to_string();
        assert!(
            err.contains("hex") || err.contains("Invalid"),
            "error was: {err}"
        );
    }

    #[test]
    fn rejects_array_form() {
        // Arrays are not a supported format; deserializer only accepts a string.
        let result = parse(r#"lane_id = [151, 177, 0, 0]"#);
        assert!(result.is_err(), "array form must be rejected");
    }

    #[test]
    fn rejects_missing_field() {
        let err = parse(r"")
            .expect_err("missing lane_id must be rejected")
            .to_string();
        assert!(
            err.contains("lane_id") || err.contains("missing"),
            "error was: {err}"
        );
    }

    #[test]
    fn validate_rejects_programmatic_native_lane() {
        let cfg = IgraConfig::from_namespace([0, 0, 0, 0]);
        let err = cfg
            .validate()
            .expect_err("programmatically constructed NATIVE lane must be rejected")
            .to_string();
        assert!(err.contains("NATIVE"), "error was: {err}");
    }

    #[test]
    fn rejects_coinbase_reserved_shape() {
        let err = parse(r#"lane_id = "01000000""#)
            .expect_err("COINBASE-shaped lane (01000000) must be rejected")
            .to_string();
        assert!(err.contains("reserved-system shape"), "error was: {err}");
    }

    #[test]
    fn rejects_registry_reserved_shape() {
        let err = parse(r#"lane_id = "02000000""#)
            .expect_err("REGISTRY-shaped lane (02000000) must be rejected")
            .to_string();
        assert!(err.contains("reserved-system shape"), "error was: {err}");
    }

    #[test]
    fn rejects_any_reserved_system_shape() {
        let err = parse(r#"lane_id = "ff000000""#)
            .expect_err("any [x, 0×19] shape must be rejected")
            .to_string();
        assert!(err.contains("reserved-system shape"), "error was: {err}");
    }

    #[test]
    fn from_namespace_zero_pads_per_kip21() {
        let cfg = IgraConfig::from_namespace([0x97, 0xb1, 0, 0]);
        let lane = cfg.lane_id();
        assert_eq!(lane[0], 0x97);
        assert_eq!(lane[1], 0xb1);
        assert!(lane[4..].iter().all(|&b| b == 0));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_programmatically_corrupted_kip21_padding() {
        // Build via the safe constructor, then poke a byte past the
        // namespace to simulate a future code path that mutates `lane_id`
        // through `pub(crate)` access. `validate` must catch the
        // shape violation.
        let mut cfg = IgraConfig::from_namespace([0x97, 0xb1, 0, 0]);
        cfg.lane_id[10] = 0xff;
        let err = cfg
            .validate()
            .expect_err("non-zero bytes past the namespace must be rejected")
            .to_string();
        assert!(err.contains("zero-padded per KIP-21"), "error was: {err}");
    }

    #[test]
    fn validate_rejects_programmatic_reserved_system_shape() {
        let cfg = IgraConfig::from_namespace([0x01, 0, 0, 0]);
        let err = cfg
            .validate()
            .expect_err("programmatic [x, 0×19] shape must be rejected")
            .to_string();
        assert!(err.contains("reserved-system shape"), "error was: {err}");
    }
}
