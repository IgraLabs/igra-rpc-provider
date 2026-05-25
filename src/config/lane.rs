//! KIP-21 IGRA lane configuration
//!
//! When `IGRA_LANE_ID` is set, every `eth_sendRawTransaction` payload-carrying
//! tx is validated to live on that lane (i.e. `tx.subnetwork_id` matches and
//! `tx.version` is Toccata-compatible) before mining and again before
//! broadcast. When unset, lane enforcement is disabled and the RPC behaves
//! as it did pre-Toccata.
//!
//! The lane id format mirrors the kaswallet daemon's `--subnetwork-id` flag:
//! the 4-byte user-lane namespace as 8 lowercase hex chars (no `0x` prefix,
//! e.g. `97b10000`). The wallet zero-pads it to the full 20-byte on-chain id
//! per KIP-21. Operators must configure the kaswallet daemon with a matching
//! `KASWALLET_SUBNETWORK_ID`; the pre-mining validator catches a mismatch on
//! the first request.
//!
//! Reserved built-in subnetworks (`coinbase`, `registry`) and the reserved
//! "system shape" `[x, 0×19]` for `x` other than NATIVE are rejected at parse
//! time so operators see a clear error rather than a mempool rejection.

use kaspa_consensus_core::subnets::{SubnetworkId, SUBNETWORK_ID_SIZE};
use serde::Deserialize;

/// KIP-21 user-lane namespace length (first 4 bytes of the 20-byte
/// SubnetworkId).
const NAMESPACE_LEN: usize = 4;

/// Hex-encoded length of the 4-byte namespace (8 chars).
const NAMESPACE_HEX_LEN: usize = NAMESPACE_LEN * 2;

/// KIP-21 IGRA lane configuration.
///
/// Post-Toccata, lane enforcement is **required by default** — a deployment
/// must either set `IGRA_LANE_ID` to the lane namespace or explicitly opt
/// out via `LANE_ENFORCEMENT_DISABLED=true` (dev/test only; production
/// startup logs a loud warning if this is taken).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LaneConfig {
    /// 4-byte lane namespace as 8 lowercase hex chars (no `0x` prefix).
    /// `None` (or omitted from config) means no lane id is configured;
    /// startup will then fail unless `enforcement_disabled` is `true`.
    #[serde(default)]
    pub lane_id: Option<String>,
    /// Explicit escape hatch: when `true` **and** `lane_id` is absent,
    /// the RPC starts with lane enforcement off (legacy native-subnetwork
    /// behavior). Intended for tests and pre-Toccata regression environments
    /// only; production should leave this `false` and configure `lane_id`.
    #[serde(default)]
    pub enforcement_disabled: bool,
}

/// Resolution of `LaneConfig` at startup. Either we have a concrete lane
/// to enforce, or the operator explicitly disabled enforcement.
#[derive(Debug, Clone)]
pub enum LaneMode {
    /// KIP-21 enforcement is on against the configured lane.
    Enforced(SubnetworkId),
    /// Enforcement is off because `LANE_ENFORCEMENT_DISABLED=true` was set
    /// without a lane id. Callers must log a loud warning at startup.
    Disabled,
}

impl LaneConfig {
    /// Construct from a hex namespace, validating eagerly.
    pub fn with_namespace(namespace_hex: &str) -> Result<Self, String> {
        parse_namespace(namespace_hex)?;
        Ok(Self {
            lane_id: Some(namespace_hex.to_string()),
            enforcement_disabled: false,
        })
    }

    /// Test/dev helper: build a `LaneConfig` with enforcement explicitly
    /// disabled. Use only from `#[cfg(test)]` or local development paths.
    pub fn disabled() -> Self {
        Self {
            lane_id: None,
            enforcement_disabled: true,
        }
    }

    /// Validate the configuration and confirm the deployment is in a sane
    /// state. Fails if `lane_id` is missing AND `enforcement_disabled` is
    /// false — i.e. the operator forgot to configure KIP-21 enforcement
    /// AND did not explicitly opt out.
    pub fn validate(&self) -> Result<(), String> {
        self.resolve().map(|_| ())
    }

    /// Parse the configured lane id into a `SubnetworkId`. `Ok(None)` means
    /// `lane_id` was absent or empty; whether that's a valid state depends
    /// on `enforcement_disabled` (see [`resolve`]).
    pub fn parsed(&self) -> Result<Option<SubnetworkId>, String> {
        match self.lane_id.as_deref() {
            None | Some("") => Ok(None),
            Some(s) => parse_namespace(s).map(Some),
        }
    }

    /// Resolve into the effective [`LaneMode`].
    ///
    /// - `Some(valid)` → [`LaneMode::Enforced`].
    /// - `None`/empty + `enforcement_disabled=true` → [`LaneMode::Disabled`].
    /// - `None`/empty + `enforcement_disabled=false` → `Err` ("required").
    pub fn resolve(&self) -> Result<LaneMode, String> {
        match (self.parsed()?, self.enforcement_disabled) {
            (Some(id), _) => Ok(LaneMode::Enforced(id)),
            (None, true) => Ok(LaneMode::Disabled),
            (None, false) => Err(
                "IGRA_LANE_ID is required; set LANE_ENFORCEMENT_DISABLED=true to \
                 explicitly opt out (dev/tests only, not for production)"
                    .to_string(),
            ),
        }
    }
}

/// Validate and parse a KIP-21 4-byte lane namespace into a full
/// 20-byte `SubnetworkId`.
///
/// Mirrors the kaswallet daemon's `parse_subnetwork_id_arg`
/// (`daemon/src/args.rs`) so operators see consistent error messages on both
/// sides. Rejects:
/// - `0x` / `0X` prefix,
/// - wrong length (must be exactly 8 lowercase hex chars),
/// - any uppercase hex char (matches the daemon's strictness),
/// - non-hex characters,
/// - the explicit NATIVE namespace `00000000` (operators who want to disable
///   enforcement should omit `IGRA_LANE_ID` entirely; an explicit NATIVE
///   would enable enforcement and then reject every v0 tx the daemon emits),
/// - reserved built-in ids (coinbase, registry),
/// - the reserved-system shape `[x, 0×19]`.
fn parse_namespace(s: &str) -> Result<SubnetworkId, String> {
    if s.starts_with("0x") || s.starts_with("0X") {
        return Err("IGRA_LANE_ID must not have a 0x prefix".to_string());
    }
    if s.len() != NAMESPACE_HEX_LEN {
        return Err(format!(
            "IGRA_LANE_ID must be the 4-byte lane namespace as \
             {NAMESPACE_HEX_LEN} lowercase hex chars (e.g. 97b10000), got {} chars",
            s.len(),
        ));
    }
    if s.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(format!(
            "IGRA_LANE_ID must be lowercase hex (e.g. 97b10000), got {s:?}"
        ));
    }
    // Decode directly into the first 4 bytes of a zero-initialised
    // SubnetworkId. The trailing 16 bytes stay zero per KIP-21's
    // `[namespace, 0×16]` shape; no String allocation, no second hex pass.
    let mut bytes = [0u8; SUBNETWORK_ID_SIZE];
    hex::decode_to_slice(s, &mut bytes[..NAMESPACE_LEN])
        .map_err(|e| format!("IGRA_LANE_ID namespace must be valid lowercase hex: {e}"))?;
    let parsed = SubnetworkId::from_bytes(bytes);
    if parsed.is_native() {
        return Err(
            "IGRA_LANE_ID: explicit NATIVE namespace 00000000 selects no lane; \
             use LANE_ENFORCEMENT_DISABLED=true to opt out of enforcement"
                .to_string(),
        );
    }
    if parsed.is_builtin() {
        return Err(format!(
            "IGRA_LANE_ID: reserved built-in subnetwork {parsed} is not allowed"
        ));
    }
    // The reserved-system shape `[x, 0×19]` is rejected by consensus as
    // `SubnetworksDisabled`. Catch it at parse time so operators see a
    // clear error rather than a mempool rejection.
    if bytes[1..NAMESPACE_LEN].iter().all(|&b| b == 0) {
        return Err(format!(
            "IGRA_LANE_ID: namespace 0x{:02x}000000 collides with the reserved \
             system shape [x, 0×19] per KIP-21",
            bytes[0],
        ));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IGRA_NAMESPACE: &str = "97b10000";

    fn cfg(lane_id: Option<&str>) -> LaneConfig {
        LaneConfig {
            lane_id: lane_id.map(String::from),
            enforcement_disabled: false,
        }
    }

    #[test]
    fn parses_4byte_namespace_and_zero_pads() {
        let parsed = cfg(Some(IGRA_NAMESPACE))
            .parsed()
            .expect("valid namespace")
            .expect("lane enabled");
        let bytes: &[u8; SUBNETWORK_ID_SIZE] = parsed.as_ref();
        assert_eq!(&bytes[..NAMESPACE_LEN], &[0x97, 0xb1, 0x00, 0x00]);
        assert!(bytes[NAMESPACE_LEN..].iter().all(|&b| b == 0));
    }

    #[test]
    fn rejects_0x_prefix() {
        let err = cfg(Some(&format!("0x{IGRA_NAMESPACE}")))
            .parsed()
            .expect_err("must reject 0x prefix");
        assert!(err.contains("0x prefix"), "got: {err}");
    }

    #[test]
    fn rejects_wrong_length() {
        for hex in ["97b1000", "97b100000", "97b1", "97b1000000"] {
            let msg = cfg(Some(hex))
                .parsed()
                .expect_err(&format!("must reject length-{} input", hex.len()));
            assert!(msg.contains("hex chars"), "got: {msg}");
        }
    }

    #[test]
    fn rejects_non_hex_chars() {
        let err = cfg(Some("zzzzzzzz"))
            .parsed()
            .expect_err("must reject non-hex");
        assert!(err.contains("hex"), "got: {err}");
    }

    #[test]
    fn rejects_builtin_coinbase() {
        let err = cfg(Some("01000000"))
            .parsed()
            .expect_err("must reject coinbase");
        assert!(err.contains("built-in"), "got: {err}");
    }

    #[test]
    fn rejects_builtin_registry() {
        let err = cfg(Some("02000000"))
            .parsed()
            .expect_err("must reject registry");
        assert!(err.contains("built-in"), "got: {err}");
    }

    #[test]
    fn rejects_reserved_system_shape() {
        // `[x, 0×19]` for x != 0/1/2 still has bytes[1..4] == 0, which
        // collides with the reserved-system shape per KIP-21.
        let err = cfg(Some("97000000"))
            .parsed()
            .expect_err("must reject system shape");
        assert!(err.contains("reserved system shape"), "got: {err}");
    }

    #[test]
    fn rejects_explicit_native_namespace() {
        // Operators sometimes try `IGRA_LANE_ID=00000000` to "disable" the
        // feature. That would enable enforcement against NATIVE and reject
        // every v0 tx the daemon emits — surprising. Fail loud at parse
        // time and point them at the actual escape hatch.
        let err = cfg(Some("00000000"))
            .parsed()
            .expect_err("must reject explicit NATIVE");
        assert!(
            err.contains("NATIVE") && err.contains("LANE_ENFORCEMENT_DISABLED"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_uppercase_hex() {
        // kaswallet daemon's parse_subnetwork_id_arg is lowercase-only;
        // match it so the two surfaces agree on accepted input.
        let err = cfg(Some("97B10000"))
            .parsed()
            .expect_err("must reject uppercase");
        assert!(err.contains("lowercase"), "got: {err}");
    }

    #[test]
    fn resolve_enforced_when_lane_id_set() {
        match cfg(Some(IGRA_NAMESPACE))
            .resolve()
            .expect("valid lane resolves")
        {
            LaneMode::Enforced(id) => {
                let bytes: &[u8] = id.as_ref();
                assert_eq!(bytes[..2], [0x97, 0xb1]);
            }
            LaneMode::Disabled => panic!("expected Enforced, got Disabled"),
        }
    }

    #[test]
    fn resolve_disabled_when_escape_hatch_set_and_no_lane_id() {
        let cfg = LaneConfig {
            lane_id: None,
            enforcement_disabled: true,
        };
        match cfg.resolve().expect("escape hatch is valid") {
            LaneMode::Disabled => {}
            LaneMode::Enforced(_) => panic!("expected Disabled, got Enforced"),
        }
    }

    #[test]
    fn resolve_fails_when_neither_lane_id_nor_escape_hatch_set() {
        // The default state — production-safe: refuses to start without
        // an explicit decision.
        let err = LaneConfig::default()
            .resolve()
            .expect_err("must require an explicit lane id or escape hatch");
        assert!(err.contains("IGRA_LANE_ID is required"), "got: {err}");
        assert!(
            err.contains("LANE_ENFORCEMENT_DISABLED"),
            "must name the escape hatch: {err}"
        );
    }

    #[test]
    fn resolve_fails_for_empty_lane_id_without_escape_hatch() {
        // Operators sometimes export IGRA_LANE_ID= to "clear" it. The
        // empty value is treated like absent — and still requires the
        // explicit opt-out.
        let cfg = LaneConfig {
            lane_id: Some(String::new()),
            enforcement_disabled: false,
        };
        let err = cfg
            .resolve()
            .expect_err("empty lane id without escape hatch must fail");
        assert!(err.contains("IGRA_LANE_ID is required"), "got: {err}");
    }

    #[test]
    fn resolve_prefers_lane_id_over_escape_hatch() {
        // If both are set, the configured lane id wins (escape hatch only
        // takes effect when no lane id is provided). Avoids a surprising
        // "you set both, we silently disabled".
        let cfg = LaneConfig {
            lane_id: Some(IGRA_NAMESPACE.to_string()),
            enforcement_disabled: true,
        };
        match cfg.resolve().expect("both set is valid") {
            LaneMode::Enforced(_) => {}
            LaneMode::Disabled => panic!("lane_id must take precedence over escape hatch"),
        }
    }

    #[test]
    fn validate_runs_resolve() {
        // validate() is a thin wrapper that exercises the same
        // required-by-default check; AppConfig::validate calls it at
        // startup.
        assert!(LaneConfig::default().validate().is_err());
        assert!(LaneConfig::disabled().validate().is_ok());
        assert!(cfg(Some(IGRA_NAMESPACE)).validate().is_ok());
    }

    #[test]
    fn disabled_constructor_yields_explicit_opt_out() {
        let c = LaneConfig::disabled();
        assert!(c.lane_id.is_none());
        assert!(c.enforcement_disabled);
        assert!(matches!(c.resolve(), Ok(LaneMode::Disabled)));
    }

    #[test]
    fn with_namespace_constructs_valid_config() {
        let c = LaneConfig::with_namespace(IGRA_NAMESPACE).expect("valid namespace must construct");
        let parsed = c.parsed().expect("valid").expect("enabled");
        let bytes: &[u8] = parsed.as_ref();
        assert_eq!(bytes[0], 0x97);
        assert_eq!(bytes[1], 0xb1);
    }

    #[test]
    fn with_namespace_rejects_invalid_input() {
        assert!(LaneConfig::with_namespace("0x97b10000").is_err());
        assert!(LaneConfig::with_namespace("zzzzzzzz").is_err());
    }
}
