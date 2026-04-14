use once_cell::sync::Lazy;
use std::collections::HashSet;

/// A global whitelist of allowed JSON-RPC methods.
/// This list only includes standard methods supported by providers like Alchemy and Infura.
pub static ALLOWED_METHODS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut methods = HashSet::new();

    // Standard eth_ methods supported by most providers
    methods.insert("eth_accounts");
    methods.insert("eth_blockNumber");
    methods.insert("eth_call");
    methods.insert("eth_chainId");
    methods.insert("eth_coinbase");
    methods.insert("eth_estimateGas");
    methods.insert("eth_feeHistory");
    methods.insert("eth_gasPrice");
    methods.insert("eth_getBalance");
    methods.insert("eth_getBlockByHash");
    methods.insert("eth_getBlockByNumber");
    methods.insert("eth_getBlockTransactionCountByHash");
    methods.insert("eth_getBlockTransactionCountByNumber");
    methods.insert("eth_getCode");
    methods.insert("eth_getFilterChanges");
    methods.insert("eth_getFilterLogs");
    methods.insert("eth_getLogs");
    methods.insert("eth_getProof");
    methods.insert("eth_getStorageAt");
    methods.insert("eth_getTransactionByBlockHashAndIndex");
    methods.insert("eth_getTransactionByBlockNumberAndIndex");
    methods.insert("eth_getTransactionByHash");
    methods.insert("eth_getRawTransactionByHash");
    methods.insert("eth_getTransactionCount");
    methods.insert("eth_getTransactionReceipt");
    methods.insert("eth_getBlockReceipts");
    methods.insert("eth_getUncleByBlockHashAndIndex");
    methods.insert("eth_getUncleByBlockNumberAndIndex");
    methods.insert("eth_getUncleCountByBlockHash");
    methods.insert("eth_getUncleCountByBlockNumber");
    methods.insert("eth_hashrate");
    methods.insert("eth_maxPriorityFeePerGas");
    methods.insert("eth_mining");
    methods.insert("eth_newBlockFilter");
    methods.insert("eth_newFilter");
    methods.insert("eth_newPendingTransactionFilter");
    methods.insert("eth_protocolVersion");
    methods.insert("eth_sendRawTransaction");
    methods.insert("eth_subscribe");
    methods.insert("eth_syncing");
    methods.insert("eth_uninstallFilter");
    methods.insert("eth_unsubscribe");

    // Standard net_ methods
    methods.insert("net_listening");
    methods.insert("net_peerCount");
    methods.insert("net_version");

    // Standard web3_ methods
    methods.insert("web3_clientVersion");
    methods.insert("web3_sha3");

    // debug methods
    methods.insert("debug_traceTransaction");
    methods.insert("debug_traceCall");

    methods
});

/// Check if an RPC method is allowed by the whitelist.
pub fn is_method_allowed(method: &str) -> bool {
    ALLOWED_METHODS.contains(method)
}

/// Check if an RPC method is a write operation that modifies state.
/// Write methods include:
/// - eth_sendRawTransaction, eth_sendTransaction
/// - All personal_* methods (account and signing operations)
/// - All admin_* methods (administrative operations)
pub fn is_write_method(method: &str) -> bool {
    matches!(method, "eth_sendRawTransaction" | "eth_sendTransaction")
        || method.starts_with("personal_")
        || method.starts_with("admin_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_write_method_eth_send_raw_transaction() {
        assert!(is_write_method("eth_sendRawTransaction"));
    }

    #[test]
    fn test_is_write_method_eth_send_transaction() {
        assert!(is_write_method("eth_sendTransaction"));
    }

    #[test]
    fn test_is_write_method_personal_methods() {
        assert!(is_write_method("personal_sign"));
        assert!(is_write_method("personal_sendTransaction"));
        assert!(is_write_method("personal_unlockAccount"));
    }

    #[test]
    fn test_is_write_method_admin_methods() {
        assert!(is_write_method("admin_addPeer"));
        assert!(is_write_method("admin_removePeer"));
        assert!(is_write_method("admin_startRPC"));
    }

    #[test]
    fn test_is_write_method_read_methods() {
        // These should all return false
        assert!(!is_write_method("eth_getBalance"));
        assert!(!is_write_method("eth_blockNumber"));
        assert!(!is_write_method("eth_call"));
        assert!(!is_write_method("eth_getCode"));
        assert!(!is_write_method("net_version"));
        assert!(!is_write_method("web3_clientVersion"));
        assert!(!is_write_method("debug_traceTransaction"));
    }

    #[test]
    fn test_is_write_method_edge_cases() {
        // Test methods that might be confused with write methods
        assert!(!is_write_method("eth_sendRaw")); // Not the full method name
        assert!(!is_write_method("personal")); // Just the prefix
        assert!(!is_write_method("admin")); // Just the prefix
        assert!(!is_write_method("eth_personal_sign")); // Not starting with personal_
    }
}
