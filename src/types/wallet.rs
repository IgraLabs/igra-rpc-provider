use kaspa_consensus_core::subnets::SubnetworkId;
use kaspa_consensus_core::tx::{
    MutableTransaction, ScriptPublicKey, Transaction, TransactionId, TransactionInput,
    TransactionOutpoint, TransactionOutput, TxInputMass, UtxoEntry,
};
use proto::kaswallet_proto as proto_types;
use thiserror::Error;

/// Type alias for SignableTransaction
pub type SignableTransaction = MutableTransaction<Transaction>;

#[derive(Debug, Error, Clone)]
pub enum KaspaWalletError {
    #[error("{0}")]
    UserInputError(String),
    #[error("{0}")]
    InternalServerError(String),
}

/// Convert proto WalletSignableTransaction to kaspa SignableTransaction for mining
pub fn proto_to_signable_transaction(
    proto_wst: &proto_types::WalletSignableTransaction,
) -> Result<SignableTransaction, KaspaWalletError> {
    let signed_tx = proto_wst
        .transaction
        .as_ref()
        .ok_or_else(|| KaspaWalletError::UserInputError("Missing transaction".to_string()))?;

    let signable_tx = match &signed_tx.signed {
        Some(proto_types::signed_transaction::Signed::Partially(st)) => st,
        Some(proto_types::signed_transaction::Signed::Fully(_)) => {
            return Err(KaspaWalletError::UserInputError(
                "Cannot mine fully signed transaction".to_string(),
            ));
        }
        None => {
            return Err(KaspaWalletError::UserInputError(
                "Missing signed transaction variant".to_string(),
            ));
        }
    };

    proto_signable_to_kaspa(signable_tx)
}

/// Convert proto SignableTransaction to kaspa SignableTransaction
fn proto_signable_to_kaspa(
    proto_st: &proto_types::SignableTransaction,
) -> Result<SignableTransaction, KaspaWalletError> {
    let proto_tx = proto_st
        .tx
        .as_ref()
        .ok_or_else(|| KaspaWalletError::UserInputError("Missing transaction".to_string()))?;

    let tx = proto_transaction_to_kaspa(proto_tx)?;

    let entries: Result<Vec<UtxoEntry>, KaspaWalletError> = proto_st
        .entries
        .iter()
        .filter_map(|opt_entry| opt_entry.entry.as_ref())
        .map(|entry| {
            let spk = if let Some(spk) = &entry.script_public_key {
                let version = spk.version.try_into().map_err(|_| {
                    KaspaWalletError::InternalServerError("Invalid script version".to_string())
                })?;
                let script = hex::decode(&spk.script_public_key).map_err(|e| {
                    KaspaWalletError::InternalServerError(format!("Invalid script hex: {}", e))
                })?;
                ScriptPublicKey::new(version, script.into())
            } else {
                ScriptPublicKey::default()
            };
            // covenant_id is a Toccata addition; the wallet daemon's proto
            // does not carry it, so we always pass None on decode. The
            // round-trip is asymmetric by construction.
            Ok(UtxoEntry::new(
                entry.amount,
                spk,
                entry.block_daa_score,
                entry.is_coinbase,
                None,
            ))
        })
        .collect();
    let entries = entries?;

    let mut signable = SignableTransaction::with_entries(tx, entries);
    signable.calculated_fee = proto_st.calculated_fee;
    signable.calculated_non_contextual_masses = proto_st
        .calculated_non_contextual_masses
        .as_ref()
        .map(|masses| kaspa_consensus_core::mass::NonContextualMasses {
            compute_mass: masses.compute_mass,
            transient_mass: masses.transient_mass,
        });

    Ok(signable)
}

/// Convert proto Transaction to kaspa Transaction
fn proto_transaction_to_kaspa(
    proto_tx: &proto_types::Transaction,
) -> Result<Transaction, KaspaWalletError> {
    // Parse the version first so input construction can pick the right
    // mass-commitment field (v0 → sig_op_count, v1/Toccata → compute_budget).
    // The two are not interchangeable; emitting the wrong shape causes the
    // daemon (and consensus) to reject the tx.
    let version = proto_tx.version.try_into().map_err(|_| {
        KaspaWalletError::InternalServerError("Invalid transaction version".to_string())
    })?;
    let expects_compute_budget = TxInputMass::version_expects_compute_budget_field(version);

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
            if expects_compute_budget {
                let compute_budget = u16::try_from(input.compute_budget).map_err(|_| {
                    KaspaWalletError::InternalServerError(format!(
                        "TransactionInput.compute_budget {} exceeds u16 max",
                        input.compute_budget,
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
                        "TransactionInput.sig_op_count {} exceeds u8 max",
                        input.sig_op_count,
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
                let version = spk.version.try_into().map_err(|_| {
                    KaspaWalletError::InternalServerError("Invalid script version".to_string())
                })?;
                let script = hex::decode(&spk.script_public_key).map_err(|e| {
                    KaspaWalletError::InternalServerError(format!("Invalid script hex: {}", e))
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
                "Invalid subnetwork_id length: expected 20 bytes, got {}",
                len
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

/// Convert kaspa SignableTransaction back to proto SignableTransaction
fn kaspa_signable_to_proto(
    kaspa_st: &SignableTransaction,
) -> Result<proto_types::SignableTransaction, KaspaWalletError> {
    let proto_tx = kaspa_transaction_to_proto(&kaspa_st.tx)?;

    // The proto carries no `covenant_id` field. If a future kaspa upgrade
    // populates it on inputs, silently dropping it on the round-trip would
    // sign and broadcast a tx that diverges from what the wallet expects.
    // Fail closed in all build configurations (debug and release) so the
    // first divergence surfaces as a request error, not a mempool reject.
    let entries: Result<Vec<proto_types::OptionalUtxoEntry>, KaspaWalletError> = kaspa_st
        .entries
        .iter()
        .map(|opt_entry| {
            let entry = match opt_entry.as_ref() {
                Some(e) => e,
                None => {
                    return Ok(proto_types::OptionalUtxoEntry { entry: None });
                }
            };
            if entry.covenant_id.is_some() {
                return Err(KaspaWalletError::InternalServerError(
                    "UtxoEntry.covenant_id is set but the kaswallet proto cannot \
                     carry it; round-trip would silently drop the field"
                        .to_string(),
                ));
            }
            Ok(proto_types::OptionalUtxoEntry {
                entry: Some(proto_types::UtxoEntry {
                    amount: entry.amount,
                    script_public_key: Some(proto_types::ScriptPublicKey {
                        version: entry.script_public_key.version().into(),
                        script_public_key: hex::encode(entry.script_public_key.script()),
                    }),
                    block_daa_score: entry.block_daa_score,
                    is_coinbase: entry.is_coinbase,
                }),
            })
        })
        .collect();
    let entries = entries?;

    Ok(proto_types::SignableTransaction {
        tx: Some(proto_tx),
        entries,
        calculated_fee: kaspa_st.calculated_fee,
        calculated_non_contextual_masses: kaspa_st.calculated_non_contextual_masses.as_ref().map(
            |masses| proto_types::NonContextualMasses {
                compute_mass: masses.compute_mass,
                transient_mass: masses.transient_mass,
            },
        ),
    })
}

/// Convert kaspa Transaction to proto Transaction
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
            // Toccata: the parent tx's `version` field is authoritative on
            // the wire — v0 reads `sig_op_count`, v1 reads `compute_budget`,
            // and the "other" field is always present but ignored by the
            // consensus model on receive. Always populate both (zeroing the
            // inapplicable one) so the round-trip matches kaswallet.
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
                version: output.script_public_key.version().into(),
                script_public_key: hex::encode(output.script_public_key.script()),
            }),
        })
        .collect();

    Ok(proto_types::Transaction {
        version: kaspa_tx.version.into(),
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

/// Update proto WalletSignableTransaction with a mined kaspa SignableTransaction
pub fn update_proto_with_mined_transaction(
    proto_wst: &mut proto_types::WalletSignableTransaction,
    mined_tx: SignableTransaction,
) -> Result<(), KaspaWalletError> {
    let signed_tx = proto_wst
        .transaction
        .as_mut()
        .ok_or_else(|| KaspaWalletError::UserInputError("Missing transaction".to_string()))?;

    let proto_signable = kaspa_signable_to_proto(&mined_tx)?;

    signed_tx.signed = Some(proto_types::signed_transaction::Signed::Partially(
        proto_signable,
    ));

    Ok(())
}

/// Return the payload byte length of the inner kaspa Transaction, or 0 if
/// no inner tx is present. Used to defend against the wallet daemon
/// emitting more than one payload-carrying tx in a multi-tx response —
/// we only mine and validate the last tx, so any earlier tx with a
/// non-empty payload would be signed and broadcast unvalidated.
pub fn proto_payload_len(proto_wst: &proto_types::WalletSignableTransaction) -> usize {
    proto_wst
        .transaction
        .as_ref()
        .and_then(|st| match &st.signed {
            Some(proto_types::signed_transaction::Signed::Partially(s)) => s.tx.as_ref(),
            Some(proto_types::signed_transaction::Signed::Fully(s)) => s.tx.as_ref(),
            None => None,
        })
        .map(|tx| tx.payload.len())
        .unwrap_or(0)
}

/// Check if proto WalletSignableTransaction is partially signed (for mining)
pub fn is_partially_signed(proto_wst: &proto_types::WalletSignableTransaction) -> bool {
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
