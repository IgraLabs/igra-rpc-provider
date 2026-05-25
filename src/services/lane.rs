//! KIP-21 IGRA lane validator
//!
//! Post-Toccata, every IGRA L2 transaction submitted to Kaspa via this RPC
//! must satisfy four invariants on every broadcast:
//!
//! 1. `tx.version >= TX_VERSION_TOCCATA` (non-native subnetwork txs require
//!    v1 input mass commitments).
//! 2. `tx.subnetwork_id` equals the configured IGRA lane.
//! 3. `tx.payload` is non-empty AND larger than the nonce slot, so the
//!    IGRA L2 message isn't fully overwritten by mining.
//! 4. `tx.id()` starts with the configured `TX_ID_PREFIX` (checked only
//!    after mining has fixed the trailing nonce bytes).
//!
//! Lane construction (subnetwork id + Toccata version + correct input mass
//! commitment) is owned by the kaswallet daemon via its `--subnetwork-id` /
//! `KASWALLET_SUBNETWORK_ID` config. This module does **not** mutate
//! `subnetwork_id` or `version` — doing so post-decode would invalidate the
//! daemon-computed `calculated_non_contextual_masses` and leave inputs in
//! the wrong shape (v0 inputs carry `sig_op_count`, v1 inputs carry
//! `compute_budget`). The RPC validates the daemon-built tx and enforces
//! the prefix invariant via the existing nonce-mining loop in
//! [`crate::services::mining`].
//!
//! `validate_lane_transaction` is called twice in
//! `WalletCaller::complete_transaction_flow`:
//! - `Stage::PreMining` — immediately after `proto_to_signable_transaction`,
//!   to catch a kaswallet/RPC config mismatch before burning CPU on mining.
//!   The tx-id prefix check is skipped here (the id has not been mined yet).
//! - `Stage::PreBroadcast` — after `miner.mine_transaction()` returns and
//!   before `sign_transactions()`. All four invariants are enforced.
//!
//! Error messages returned to the JSON-RPC client name only the failing
//! invariant class (`version` / `subnetwork` / `payload` / `prefix`); the
//! full diagnostic (actual lane, expected lane, tx id, env-var hint) is
//! logged at `warn!` level for operators and never leaves the server.

use crate::error::AppError;
use crate::types::wallet::SignableTransaction;
use kaspa_consensus_core::constants::TX_VERSION_TOCCATA;
use kaspa_consensus_core::subnets::SubnetworkId;
use tracing::warn;

/// Paired lane id + non-empty tx-id prefix, validated at construction so
/// the prefix invariant can never be vacuously satisfied (`starts_with(&[])`
/// is always true). Use `Option<LaneEnforcement>` to model "enforcement
/// off"; constructing the struct itself guarantees both fields are valid.
#[derive(Debug, Clone)]
pub struct LaneEnforcement {
    lane: SubnetworkId,
    tx_id_prefix: Vec<u8>,
}

impl LaneEnforcement {
    /// Build a `LaneEnforcement`. Rejects an empty prefix — required so the
    /// post-mining check at [`Stage::PreBroadcast`] cannot be silently
    /// disabled by a misconfigured caller.
    pub fn new(lane: SubnetworkId, tx_id_prefix: Vec<u8>) -> Result<Self, String> {
        if tx_id_prefix.is_empty() {
            return Err(
                "LaneEnforcement: tx_id_prefix must be non-empty (an empty prefix \
                 makes the post-mining invariant vacuously true)"
                    .to_string(),
            );
        }
        Ok(Self { lane, tx_id_prefix })
    }

    pub fn lane(&self) -> &SubnetworkId {
        &self.lane
    }

    pub fn tx_id_prefix(&self) -> &[u8] {
        &self.tx_id_prefix
    }
}

/// Nonce slot length (bytes) appended to the IGRA payload by the mining
/// loop. Must match `mining::mine_blocking`'s `nonce_length`. A lane tx
/// whose payload is `<= NONCE_LEN` bytes has the entire L2 message
/// overwritten by mining, so we reject any payload at or below this size.
const NONCE_LEN: usize = 4;

/// Smallest payload a lane tx may carry. One byte of L2 data + the
/// 4-byte nonce slot — anything shorter is irrecoverably truncated by
/// the mining loop.
const MIN_LANE_PAYLOAD_LEN: usize = NONCE_LEN + 1;

/// Stage at which lane validation is being performed. Drives whether the
/// final-tx-id prefix is checked and shapes error messages so operators can
/// tell pre-mining (daemon misconfiguration) from pre-broadcast (post-mining
/// drift) failures in their logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Before nonce mining. Tx id has not yet been finalized for prefix
    /// matching; only version, subnetwork id, and payload presence are
    /// checked.
    PreMining,
    /// After mining, immediately before signing and broadcast. All four
    /// invariants — version, subnetwork id, payload presence, tx-id
    /// prefix — are enforced.
    PreBroadcast,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Stage::PreMining => "pre-mining",
            Stage::PreBroadcast => "pre-broadcast",
        }
    }
}

/// Enforce the four KIP-21 lane invariants on `tx`.
///
/// On violation, logs the rich diagnostic at `warn!` (lane, tx id, env-var
/// hint) and returns [`AppError::LaneEnforcementFailed`] carrying only the
/// failing invariant class — never the configured values or env-var names.
/// The client gets enough to differentiate "your tx" from "operator
/// misconfig"; the operator gets the full picture in the server log.
pub fn validate_lane_transaction(
    tx: &SignableTransaction,
    expected_lane: &SubnetworkId,
    tx_id_prefix: &[u8],
    stage: Stage,
) -> Result<(), AppError> {
    let stage_name = stage.as_str();

    // 1. Toccata-compatible version. Non-native subnetwork txs require
    //    TX_VERSION_TOCCATA (v1) per consensus; v0 inputs carry the wrong
    //    mass-commitment field.
    if tx.tx.version < TX_VERSION_TOCCATA {
        warn!(
            stage = stage_name,
            tx_version = tx.tx.version,
            required_version = TX_VERSION_TOCCATA,
            "lane enforcement failed: pre-Toccata tx version"
        );
        return Err(AppError::LaneEnforcementFailed(format!(
            "{stage_name}: version"
        )));
    }

    // 2. Subnetwork id matches the configured lane. A failure at PreMining
    //    almost always means KASWALLET_SUBNETWORK_ID and IGRA_LANE_ID
    //    disagree; the operator log spells both out.
    if &tx.tx.subnetwork_id != expected_lane {
        warn!(
            stage = stage_name,
            tx_subnetwork_id = %tx.tx.subnetwork_id,
            expected_lane = %expected_lane,
            "lane enforcement failed: subnetwork mismatch \
             (check KASWALLET_SUBNETWORK_ID matches IGRA_LANE_ID)"
        );
        return Err(AppError::LaneEnforcementFailed(format!(
            "{stage_name}: subnetwork"
        )));
    }

    // 3. Payload must be larger than the nonce slot — otherwise the mining
    //    loop overwrites the entire L2 message. Defense against an upstream
    //    constructor emitting a payload at or below NONCE_LEN bytes; current
    //    IGRA payload construction is always well above this.
    if tx.tx.payload.len() < MIN_LANE_PAYLOAD_LEN {
        warn!(
            stage = stage_name,
            payload_len = tx.tx.payload.len(),
            min_required = MIN_LANE_PAYLOAD_LEN,
            "lane enforcement failed: payload too short — mining would \
             overwrite all L2 data"
        );
        return Err(AppError::LaneEnforcementFailed(format!(
            "{stage_name}: payload"
        )));
    }

    // 4. Final tx id matches the configured TX_ID_PREFIX. Only meaningful
    //    after mining has fixed the trailing nonce bytes.
    if stage == Stage::PreBroadcast && !tx.id().as_bytes().starts_with(tx_id_prefix) {
        warn!(
            stage = stage_name,
            tx_id = %tx.id(),
            expected_prefix = format!("0x{}", hex::encode(tx_id_prefix)),
            "lane enforcement failed: tx id does not start with configured prefix"
        );
        return Err(AppError::LaneEnforcementFailed(
            "pre-broadcast: prefix".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaspa_consensus_core::constants::TX_VERSION;
    use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
    use kaspa_consensus_core::subnets::SUBNETWORK_ID_SIZE;
    use kaspa_consensus_core::tx::Transaction;

    /// Arbitrary non-native, non-builtin SubnetworkId used as "the
    /// configured IGRA lane" in tests. The concrete bytes are immaterial
    /// to the validator — what matters is that it isn't NATIVE/COINBASE/
    /// REGISTRY and that it differs from [`other_test_lane`]. Decoupled
    /// from any real deployment value so changing the production lane
    /// namespace never has to touch this file.
    fn test_lane() -> SubnetworkId {
        SubnetworkId::from_bytes([0xaa; SUBNETWORK_ID_SIZE])
    }

    /// A second distinct test lane, used to verify the subnetwork-mismatch
    /// branch.
    fn other_test_lane() -> SubnetworkId {
        SubnetworkId::from_bytes([0xbb; SUBNETWORK_ID_SIZE])
    }

    /// Build a SignableTransaction with the given version, lane, and payload.
    /// Calls `finalize()` so the cached tx id reflects the chosen fields.
    fn make_tx(version: u16, lane: SubnetworkId, payload: Vec<u8>) -> SignableTransaction {
        let mut tx = Transaction::new(version, vec![], vec![], 0, lane, 0, payload);
        tx.finalize();
        SignableTransaction::new(tx)
    }

    const ZERO_PREFIX: &[u8] = &[0x00];
    // 5 bytes: 1 byte of L2 data + 4-byte nonce slot. Satisfies the
    // MIN_LANE_PAYLOAD_LEN invariant in tests that exercise other rules.
    const VALID_PAYLOAD: &[u8] = &[0xab, 0, 0, 0, 0];

    #[test]
    fn pre_mining_accepts_correct_lane_tx() {
        let tx = make_tx(TX_VERSION_TOCCATA, test_lane(), VALID_PAYLOAD.to_vec());
        validate_lane_transaction(&tx, &test_lane(), &[0xff], Stage::PreMining)
            .expect("pre-mining ignores prefix; the other invariants hold");
    }

    #[test]
    fn pre_mining_does_not_check_prefix() {
        // Tx is correctly on the lane and Toccata-versioned but its id will
        // almost certainly not start with [0xff]. PreMining must still pass.
        let tx = make_tx(TX_VERSION_TOCCATA, test_lane(), VALID_PAYLOAD.to_vec());
        let id_prefix = tx.id().as_bytes()[0];
        let mismatching_prefix = vec![id_prefix.wrapping_add(1)];
        validate_lane_transaction(&tx, &test_lane(), &mismatching_prefix, Stage::PreMining)
            .expect("pre-mining must skip prefix check");
    }

    #[test]
    fn pre_broadcast_accepts_lane_tx_with_matching_prefix() {
        // Hand-mine: search a few nonce values for an id starting with 0x00.
        // Bounded loop — 65k iterations; matches typically hit within 256
        // tries for a 1-byte prefix. Payload is 5 bytes: 1 byte of L2 data
        // + 4-byte nonce slot (last 4 bytes), matching production layout.
        let lane = test_lane();
        let mut payload = VALID_PAYLOAD.to_vec();
        let mut tx_built = None;
        for nonce in 0u32..u32::from(u16::MAX) {
            let nonce_start = payload.len() - NONCE_LEN;
            payload[nonce_start..].copy_from_slice(&nonce.to_be_bytes());
            let mut t = Transaction::new(
                TX_VERSION_TOCCATA,
                vec![],
                vec![],
                0,
                lane,
                0,
                payload.clone(),
            );
            t.finalize();
            if t.id().as_bytes().starts_with(ZERO_PREFIX) {
                tx_built = Some(SignableTransaction::new(t));
                break;
            }
        }
        let tx = tx_built.expect("must find a 1-byte prefix within 65k iterations");
        validate_lane_transaction(&tx, &lane, ZERO_PREFIX, Stage::PreBroadcast)
            .expect("all four invariants hold");
    }

    #[test]
    fn rejects_pre_toccata_version() {
        let tx = make_tx(TX_VERSION, test_lane(), VALID_PAYLOAD.to_vec());
        let err = validate_lane_transaction(&tx, &test_lane(), ZERO_PREFIX, Stage::PreMining)
            .expect_err("v0 tx on a lane must be rejected");
        // Client-facing message names only the failing invariant class.
        // Full diagnostic (actual vs. required version) is logged via
        // tracing::warn! on the server.
        let msg = err.to_string();
        assert!(msg.contains("version"), "must name 'version' class: {msg}");
    }

    #[test]
    fn rejects_wrong_subnetwork() {
        let tx = make_tx(
            TX_VERSION_TOCCATA,
            other_test_lane(),
            VALID_PAYLOAD.to_vec(),
        );
        let err = validate_lane_transaction(&tx, &test_lane(), ZERO_PREFIX, Stage::PreMining)
            .expect_err("tx on the wrong lane must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("subnetwork"),
            "must name 'subnetwork' class: {msg}"
        );
        // Actual lane values and KASWALLET_SUBNETWORK_ID / IGRA_LANE_ID
        // hints stay in the server warn! log — they must NOT leak to the
        // RPC client.
        assert!(
            !msg.contains("KASWALLET_SUBNETWORK_ID"),
            "must not leak env var name to client: {msg}"
        );
        assert!(
            !msg.contains(&test_lane().to_string()),
            "must not leak configured lane to client: {msg}"
        );
    }

    #[test]
    fn rejects_empty_payload() {
        let tx = make_tx(TX_VERSION_TOCCATA, test_lane(), vec![]);
        let err = validate_lane_transaction(&tx, &test_lane(), ZERO_PREFIX, Stage::PreMining)
            .expect_err("lane tx with no payload must be rejected");
        assert!(
            err.to_string().contains("payload"),
            "must name 'payload' class: {err}"
        );
    }

    #[test]
    fn rejects_payload_too_short_for_mining_nonce() {
        // Payload of exactly NONCE_LEN bytes would have the entire L2 data
        // overwritten by the mining loop. Catch at pre-mining so we never
        // burn CPU producing a tx with a destroyed L2 message.
        let tx = make_tx(TX_VERSION_TOCCATA, test_lane(), vec![1u8; NONCE_LEN]);
        let err = validate_lane_transaction(&tx, &test_lane(), ZERO_PREFIX, Stage::PreMining)
            .expect_err("payload <= NONCE_LEN must be rejected");
        assert!(
            err.to_string().contains("payload"),
            "must name 'payload' class: {err}"
        );
    }

    #[test]
    fn pre_broadcast_rejects_lane_tx_without_prefix() {
        // Correct lane + version + payload, but tx id (almost certainly)
        // does not start with [0xff, 0xff].
        let tx = make_tx(TX_VERSION_TOCCATA, test_lane(), VALID_PAYLOAD.to_vec());
        let unmineable_prefix = [0xff, 0xff];
        let err =
            validate_lane_transaction(&tx, &test_lane(), &unmineable_prefix, Stage::PreBroadcast)
                .expect_err("lane tx without prefix must be rejected at pre-broadcast");
        let msg = err.to_string();
        assert!(msg.contains("prefix"), "must name 'prefix' class: {msg}");
        assert!(
            !msg.contains(&hex::encode(unmineable_prefix)),
            "must not leak configured prefix to client: {msg}"
        );
    }

    #[test]
    fn lane_enforcement_rejects_empty_prefix() {
        let err = LaneEnforcement::new(test_lane(), vec![])
            .expect_err("empty prefix must be rejected at construction");
        assert!(err.contains("non-empty"), "got: {err}");
    }

    #[test]
    fn lane_enforcement_constructs_with_valid_prefix() {
        let e = LaneEnforcement::new(test_lane(), vec![0x97, 0xb1])
            .expect("valid prefix must construct");
        assert_eq!(e.lane(), &test_lane());
        assert_eq!(e.tx_id_prefix(), &[0x97, 0xb1]);
    }

    #[test]
    fn native_subnetwork_tx_is_rejected_when_lane_enforcement_is_on() {
        // If the daemon is misconfigured (KASWALLET_SUBNETWORK_ID unset)
        // and emits a native-subnetwork tx, but the RPC has lane
        // enforcement on, validation must catch this on the very first
        // request before any mining work.
        let tx = make_tx(TX_VERSION, SUBNETWORK_ID_NATIVE, VALID_PAYLOAD.to_vec());
        let err = validate_lane_transaction(&tx, &test_lane(), ZERO_PREFIX, Stage::PreMining)
            .expect_err("native tx must be rejected when lane is configured");
        // Either version (v0) or subnetwork (NATIVE) class fires first.
        let msg = err.to_string();
        assert!(
            msg.contains("version") || msg.contains("subnetwork"),
            "must name a concrete failure class: {msg}"
        );
    }
}
