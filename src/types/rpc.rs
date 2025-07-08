use ethers::types::U256;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const NONCE_SIZE: usize = 4;

/// Represents a JSON-RPC request.
///
/// A JSON-RPC request consists of the `jsonrpc` version, the method being
/// called, any associated parameters (`params`), and a unique ID (`id`).
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcRequest {
    /// The JSON-RPC protocol version, typically "2.0".
    pub jsonrpc: String,
    /// The method name to be invoked.
    pub method: String,
    /// The parameters for the method as a JSON value.
    pub params: Value,
    /// The identifier for the request, used to match with a response.
    pub id: Value,
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
