use proto::kaswallet_proto as proto_types;

// Test-only imports and the `KaspaWalletError` type backing the
// proto/kaspa round-trip converters below. The runtime lane-validation
// path inspects proto values directly and never needs a kaspa-native
// `Transaction`, so none of this leaks into production builds.
#[cfg(test)]
use {
    kaspa_consensus_core::{
        subnets::SubnetworkId,
        tx::{
            MutableTransaction, ScriptPublicKey, Transaction, TransactionId, TransactionInput,
            TransactionOutpoint, TransactionOutput, TxInputMass,
        },
    },
    thiserror::Error,
};

#[cfg(test)]
pub type SignableTransaction = MutableTransaction<Transaction>;

#[cfg(test)]
#[derive(Debug, Error, Clone)]
pub enum KaspaWalletError {
    #[error("{0}")]
    UserInputError(String),
    #[error("{0}")]
    InternalServerError(String),
}

/// Convert proto Transaction to kaspa Transaction.
///
/// Per-input mass is dispatched by `tx.version`:
/// - v0: `sig_op_count` (u8) is authoritative; `compute_budget` ignored.
/// - v1+ (Toccata): `compute_budget` (u16) is authoritative; `sig_op_count`
///   ignored.
///
/// This mirrors kaswallet's `transaction_input_from_proto` in
/// `common/src/proto_convert.rs`. Currently used only by tests; the
/// runtime path validates the wire-side proto directly via
/// `WalletCaller::validate_lane_transaction`.
#[cfg(test)]
fn proto_transaction_to_kaspa(
    proto_tx: &proto_types::Transaction,
) -> Result<Transaction, KaspaWalletError> {
    let version = u16::try_from(proto_tx.version).map_err(|_| {
        KaspaWalletError::InternalServerError("Invalid transaction version".to_string())
    })?;
    let inputs_use_compute_budget = TxInputMass::version_expects_compute_budget_field(version);

    let inputs: Result<Vec<TransactionInput>, KaspaWalletError> = proto_tx
        .inputs
        .iter()
        .map(|input| {
            let outpoint = input.previous_outpoint.as_ref().map_or_else(
                || TransactionOutpoint::new(TransactionId::default(), 0),
                |op| {
                    let tx_id = TransactionId::from_slice(op.transaction_id.as_ref());
                    TransactionOutpoint::new(tx_id, op.index)
                },
            );
            if inputs_use_compute_budget {
                let compute_budget = u16::try_from(input.compute_budget).map_err(|_| {
                    KaspaWalletError::InternalServerError(format!(
                        "compute_budget {} exceeds u16::MAX",
                        input.compute_budget
                    ))
                })?;
                Ok(TransactionInput::new_with_compute_budget(
                    outpoint,
                    input.signature_script.to_vec(),
                    input.sequence,
                    compute_budget,
                ))
            } else {
                let sig_op_count = u8::try_from(input.sig_op_count).map_err(|_| {
                    KaspaWalletError::InternalServerError(format!(
                        "sig_op_count {} exceeds u8::MAX",
                        input.sig_op_count
                    ))
                })?;
                Ok(TransactionInput::new(
                    outpoint,
                    input.signature_script.to_vec(),
                    input.sequence,
                    sig_op_count,
                ))
            }
        })
        .collect();
    let inputs = inputs?;

    let outputs: Result<Vec<TransactionOutput>, KaspaWalletError> = proto_tx
        .outputs
        .iter()
        .map(|output| {
            let spk = if let Some(spk) = &output.script_public_key {
                let version = u16::try_from(spk.version).map_err(|_| {
                    KaspaWalletError::InternalServerError("Invalid script version".to_string())
                })?;
                let script = hex::decode(&spk.script_public_key).map_err(|e| {
                    KaspaWalletError::InternalServerError(format!("Invalid script hex: {e}"))
                })?;
                ScriptPublicKey::new(version, script.into())
            } else {
                ScriptPublicKey::default()
            };
            Ok(TransactionOutput::new(output.value, spk))
        })
        .collect();
    let outputs = outputs?;

    let subnetwork_id = match proto_tx.subnetwork_id.len() {
        20 => {
            let mut arr = [0u8; 20];
            arr.copy_from_slice(&proto_tx.subnetwork_id);
            SubnetworkId::from_bytes(arr)
        }
        0 => SubnetworkId::from_bytes([0u8; 20]),
        len => {
            return Err(KaspaWalletError::InternalServerError(format!(
                "Invalid subnetwork_id length: expected 20 bytes, got {len}"
            )));
        }
    };

    Ok(Transaction::new(
        version,
        inputs,
        outputs,
        proto_tx.lock_time,
        subnetwork_id,
        proto_tx.gas,
        proto_tx.payload.to_vec(),
    ))
}

/// Convert a kaspa `Transaction` to its proto representation.
///
/// Both per-input mass fields are always emitted; the parent
/// `Transaction.version` tells the receiver which one is authoritative.
/// Mirrors kaswallet's `transaction_input_to_proto` in
/// `common/src/proto_convert.rs`. Currently used only by tests.
#[cfg(test)]
fn kaspa_transaction_to_proto(
    kaspa_tx: &Transaction,
) -> Result<proto_types::Transaction, KaspaWalletError> {
    let inputs: Vec<proto_types::TransactionInput> = kaspa_tx
        .inputs
        .iter()
        .map(|input| proto_types::TransactionInput {
            previous_outpoint: Some(proto_types::TransactionOutpoint {
                transaction_id: AsRef::<[u8]>::as_ref(&input.previous_outpoint.transaction_id)
                    .to_vec()
                    .into(),
                index: input.previous_outpoint.index,
            }),
            signature_script: input.signature_script.clone().into(),
            sequence: input.sequence,
            sig_op_count: u32::from(input.mass.sig_op_count().unwrap_or(0)),
            compute_budget: u32::from(input.mass.compute_budget().unwrap_or(0)),
        })
        .collect();

    let outputs: Vec<proto_types::TransactionOutput> = kaspa_tx
        .outputs
        .iter()
        .map(|output| proto_types::TransactionOutput {
            value: output.value,
            script_public_key: Some(proto_types::ScriptPublicKey {
                version: u32::from(output.script_public_key.version()),
                script_public_key: hex::encode(output.script_public_key.script()),
            }),
        })
        .collect();

    Ok(proto_types::Transaction {
        version: u32::from(kaspa_tx.version),
        inputs,
        outputs,
        lock_time: kaspa_tx.lock_time,
        subnetwork_id: AsRef::<[u8]>::as_ref(&kaspa_tx.subnetwork_id)
            .to_vec()
            .into(),
        gas: kaspa_tx.gas,
        payload: kaspa_tx.payload.clone().into(),
        mass: kaspa_tx.mass(),
        id: AsRef::<[u8]>::as_ref(&kaspa_tx.id()).to_vec().into(),
    })
}

/// Check if proto WalletSignableTransaction is partially signed
pub(crate) fn is_partially_signed(proto_wst: &proto_types::WalletSignableTransaction) -> bool {
    proto_wst
        .transaction
        .as_ref()
        .map(|st| {
            matches!(
                st.signed,
                Some(proto_types::signed_transaction::Signed::Partially(_))
            )
        })
        .unwrap_or(false)
}

/// Extract a reference to the underlying proto `Transaction` for the
/// `Partially` signed variant of a `WalletSignableTransaction`.
///
/// Returns `None` if the input is missing the transaction wrapper, missing
/// the signed variant, or is `Fully` signed.
pub(crate) fn partial_proto_transaction(
    proto_wst: &proto_types::WalletSignableTransaction,
) -> Option<&proto_types::Transaction> {
    let st = proto_wst.transaction.as_ref()?;
    match st.signed.as_ref()? {
        proto_types::signed_transaction::Signed::Partially(signable) => signable.tx.as_ref(),
        proto_types::signed_transaction::Signed::Fully(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaspa_consensus_core::mass::{ComputeBudget, SigopCount};
    use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
    use kaspa_hashes::Hash;

    fn fixture_outpoint() -> TransactionOutpoint {
        TransactionOutpoint::new(Hash::from_bytes([7u8; 32]), 0)
    }

    fn proto_outpoint() -> proto_types::TransactionOutpoint {
        proto_types::TransactionOutpoint {
            transaction_id: Hash::from_bytes([7u8; 32]).as_bytes().to_vec().into(),
            index: 0,
        }
    }

    fn make_proto_tx(
        version: u32,
        inputs: Vec<proto_types::TransactionInput>,
    ) -> proto_types::Transaction {
        proto_types::Transaction {
            version,
            inputs,
            outputs: vec![],
            lock_time: 0,
            subnetwork_id: AsRef::<[u8]>::as_ref(&SUBNETWORK_ID_NATIVE).to_vec().into(),
            gas: 0,
            payload: vec![].into(),
            mass: 0,
            id: vec![].into(),
        }
    }

    #[test]
    fn v1_compute_budget_input_roundtrips_through_proto() {
        // Build a v1 kaspa tx with one ComputeBudget input.
        let cb_value: u16 = 0xABCD;
        let kaspa_input = TransactionInput::new_with_compute_budget(
            fixture_outpoint(),
            vec![0xAA, 0xBB],
            42,
            cb_value,
        );
        let kaspa_tx = Transaction::new(
            1,
            vec![kaspa_input],
            vec![],
            0,
            SUBNETWORK_ID_NATIVE,
            0,
            vec![],
        );

        // kaspa -> proto: both fields present, only compute_budget non-zero.
        let proto = kaspa_transaction_to_proto(&kaspa_tx).expect("encode v1 tx");
        assert_eq!(proto.inputs.len(), 1);
        assert_eq!(proto.inputs[0].compute_budget, u32::from(cb_value));
        assert_eq!(proto.inputs[0].sig_op_count, 0);

        // proto -> kaspa: version 1 picks compute_budget.
        let decoded = proto_transaction_to_kaspa(&proto).expect("decode v1 tx");
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.inputs.len(), 1);
        match decoded.inputs[0].mass {
            TxInputMass::ComputeBudget(ComputeBudget(value)) => assert_eq!(value, cb_value),
            other => panic!("expected ComputeBudget, got {other:?}"),
        }
    }

    #[test]
    fn v0_sig_op_count_input_roundtrips_through_proto() {
        let sig_op: u8 = 5;
        let kaspa_input = TransactionInput::new(fixture_outpoint(), vec![], 99, sig_op);
        let kaspa_tx = Transaction::new(
            0,
            vec![kaspa_input],
            vec![],
            0,
            SUBNETWORK_ID_NATIVE,
            0,
            vec![],
        );

        let proto = kaspa_transaction_to_proto(&kaspa_tx).expect("encode v0 tx");
        assert_eq!(proto.inputs.len(), 1);
        assert_eq!(proto.inputs[0].sig_op_count, u32::from(sig_op));
        assert_eq!(proto.inputs[0].compute_budget, 0);

        let decoded = proto_transaction_to_kaspa(&proto).expect("decode v0 tx");
        assert_eq!(decoded.version, 0);
        match decoded.inputs[0].mass {
            TxInputMass::SigopCount(SigopCount(value)) => assert_eq!(value, sig_op),
            other => panic!("expected SigopCount, got {other:?}"),
        }
    }

    #[test]
    fn from_proto_picks_mass_field_by_tx_version() {
        // Both fields populated on the wire; consumer's version decides.
        let proto_input = proto_types::TransactionInput {
            previous_outpoint: Some(proto_outpoint()),
            signature_script: vec![].into(),
            sequence: 0,
            sig_op_count: 7,
            compute_budget: 11,
        };

        let v0_tx = make_proto_tx(0, vec![proto_input.clone()]);
        let decoded_v0 = proto_transaction_to_kaspa(&v0_tx).expect("decode v0");
        assert_eq!(decoded_v0.inputs[0].mass.sig_op_count(), Some(7));
        assert_eq!(decoded_v0.inputs[0].mass.compute_budget(), None);

        let v1_tx = make_proto_tx(1, vec![proto_input]);
        let decoded_v1 = proto_transaction_to_kaspa(&v1_tx).expect("decode v1");
        assert_eq!(decoded_v1.inputs[0].mass.compute_budget(), Some(11));
        assert_eq!(decoded_v1.inputs[0].mass.sig_op_count(), None);
    }

    #[test]
    fn v1_input_with_compute_budget_overflow_is_rejected() {
        let proto_input = proto_types::TransactionInput {
            previous_outpoint: Some(proto_outpoint()),
            signature_script: vec![].into(),
            sequence: 0,
            sig_op_count: 0,
            compute_budget: u32::from(u16::MAX).saturating_add(1),
        };
        let proto_tx = make_proto_tx(1, vec![proto_input]);
        let err = proto_transaction_to_kaspa(&proto_tx)
            .expect_err("compute_budget > u16::MAX must be rejected");
        match err {
            KaspaWalletError::InternalServerError(msg) => {
                assert!(msg.contains("compute_budget"), "msg was: {msg}");
                assert!(msg.contains("u16::MAX"), "msg was: {msg}");
            }
            other => panic!("expected InternalServerError, got {other:?}"),
        }
    }

    #[test]
    fn v0_input_with_sig_op_count_overflow_is_rejected() {
        let proto_input = proto_types::TransactionInput {
            previous_outpoint: Some(proto_outpoint()),
            signature_script: vec![].into(),
            sequence: 0,
            sig_op_count: u32::from(u8::MAX).saturating_add(1),
            compute_budget: 0,
        };
        let proto_tx = make_proto_tx(0, vec![proto_input]);
        let err = proto_transaction_to_kaspa(&proto_tx)
            .expect_err("sig_op_count > u8::MAX must be rejected");
        match err {
            KaspaWalletError::InternalServerError(msg) => {
                assert!(msg.contains("sig_op_count"), "msg was: {msg}");
                assert!(msg.contains("u8::MAX"), "msg was: {msg}");
            }
            other => panic!("expected InternalServerError, got {other:?}"),
        }
    }

    #[test]
    fn is_partially_signed_recognizes_partial_variant() {
        let signable = proto_types::SignableTransaction {
            tx: Some(make_proto_tx(1, vec![])),
            entries: vec![],
            calculated_fee: None,
            calculated_non_contextual_masses: None,
        };
        let wst = proto_types::WalletSignableTransaction {
            transaction: Some(proto_types::SignedTransaction {
                signed: Some(proto_types::signed_transaction::Signed::Partially(signable)),
            }),
            derivation_paths: vec![],
            address_by_input_index: vec![],
            address_by_output_index: vec![],
        };
        assert!(is_partially_signed(&wst));
        assert!(partial_proto_transaction(&wst).is_some());
    }

    #[test]
    fn is_partially_signed_rejects_fully_variant() {
        let signable = proto_types::SignableTransaction {
            tx: Some(make_proto_tx(1, vec![])),
            entries: vec![],
            calculated_fee: None,
            calculated_non_contextual_masses: None,
        };
        let wst = proto_types::WalletSignableTransaction {
            transaction: Some(proto_types::SignedTransaction {
                signed: Some(proto_types::signed_transaction::Signed::Fully(signable)),
            }),
            derivation_paths: vec![],
            address_by_input_index: vec![],
            address_by_output_index: vec![],
        };
        assert!(!is_partially_signed(&wst));
        assert!(partial_proto_transaction(&wst).is_none());
    }
}
