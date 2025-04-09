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
    methods.insert("eth_getStorageAt");
    methods.insert("eth_getTransactionByBlockHashAndIndex");
    methods.insert("eth_getTransactionByBlockNumberAndIndex");
    methods.insert("eth_getTransactionByHash");
    methods.insert("eth_getTransactionCount");
    methods.insert("eth_getTransactionReceipt");
    methods.insert("eth_getUncleByBlockHashAndIndex");
    methods.insert("eth_getUncleByBlockNumberAndIndex");
    methods.insert("eth_getUncleCountByBlockHash");
    methods.insert("eth_getUncleCountByBlockNumber");
    methods.insert("eth_hashrate");
    methods.insert("eth_mining");
    methods.insert("eth_newBlockFilter");
    methods.insert("eth_newFilter");
    methods.insert("eth_newPendingTransactionFilter");
    methods.insert("eth_protocolVersion");
    methods.insert("eth_sendRawTransaction");
    methods.insert("eth_syncing");
    methods.insert("eth_uninstallFilter");

    // Standard net_ methods
    methods.insert("net_listening");
    methods.insert("net_peerCount");
    methods.insert("net_version");

    // Standard web3_ methods
    methods.insert("web3_clientVersion");
    methods.insert("web3_sha3");

    methods
});

/// Check if an RPC method is allowed by the whitelist.
pub fn is_method_allowed(method: &str) -> bool {
    ALLOWED_METHODS.contains(method)
}
