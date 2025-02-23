# JSON-RPC Proxy for IGRA Execution Layer Client

## Overview

The **IGRA RPC Provider** (this application) acts as an **intermediary JSON-RPC server** for Ethereum wallets (e.g., MetaMask), forwarding most requests to the IGRA Execution Layer (EL) Client while **handling `eth_sendRawTransaction` requests separately**.

It replaces the conventional RPC Provider (e.g., a local EVM node or a service like Infura) used by L2 wallets by bridging requests between L2 wallets and two distinct components:
- The **IGRA EL Client** for all "read-only" JSON-RPC requests (reading blockchain data).
- The **KASPA Wallet** for sending transactions.

## IGRA Architecture Overview

IGRA is an EVM-compatible Layer 2 whose state is entirely defined by the Base Layer — KASPA DAG — through its transaction history. Because of this dependency:

- L2 transactions cannot be sent directly to the IGRA EL Client. Instead, they must first be included in an L1 transaction on the Base Layer (KASPA).  
- Users send transactions via (the IGRA version of) the KASPA Wallet. When an L1 transaction containing the L2 payload is minted on the Base Layer, a component called **Viaduct** detects it. Viaduct then interacts with the **IGRA Block Builder**, which in turn communicates with the IGRA EL Client (essentially an IGRA version of the `reth` Ethereum EL node) to execute the L2 transaction and update its internal state.
- Standard L2 wallets (e.g., MetaMask) typically interact with EVM nodes using the Ethereum JSON-RPC interface** for both reading data and sending transactions. However, in IGRA’s architecture, sending a transaction requires an additional step: the transaction must be relayed through the Base Layer.

Therefore, the **IGRA RPC Provider**:
- Acts as a transparent proxy for L2 wallets by forwarding "read-only" JSON-RPC requests (such as `eth_chainId` and `eth_getTransaction`) to the IGRA EL Client.
- Redirects transaction submissions (`eth_sendRawTransaction` requests) to the KASPA Wallet, ensuring that L2 transactions are properly included on the Base Layer.

Currently, (the IGRA version of) the KASPA Wallet only supports a CLI interface. Consequently, the handler for `eth_sendRawTransaction` calls a configurable shell command to instruct the KASPA Wallet on sending a transaction with the L2 payload to KASPA DAG. This interface is planned for future improvements.

## Features
✅ **Proxy Mode**: Transparently forwards "read-only" JSON-RPC requests (`eth_blockNumber`, `eth_getBalance`, etc.) to the IGRA EL client.  
✅ **Custom Handling for `eth_sendRawTransaction`**:
  - Calls the KASPA Wallet for transaction submission to the Base Layer (KASPA DAG).
  - Returns transaction hash only if all checks pass and the transaction gets submitted.  
✅ Structured Logging: Uses `tracing` for detailed logs.  
✅ Error Handling: Mimics standard Ethereum JSON-RPC error responses.  
---

## 🚀 Installation & Setup

### **1️⃣ Install Rust (if not already installed)**
```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### **2️⃣ Clone the repository**
```sh
git clone https://github.com/IgraLabs/igra-rpc-provider.git
cd igra-rpc-provider
```

### **3️⃣ Build the project**
```sh
cargo build --release
```

### **4️⃣ Run the server**
```sh
cargo run
```
By default, the server listens on **`127.0.0.1:8545`**.

---

## 📡 JSON-RPC API

### **"Read-only" Requests (Proxy Mode)**
All requests with the supported JSON-RPC methods, except `eth_sendRawTransaction`, are forwarded directly to the IGRA EL Client.
It includes the following methods (the list is incomplete).
- `eth_blockNumber`
- `eth_call`
- `eth_chainId`
- `eth_estimateGas`
- `eth_getBlockByNumber`
- `eth_getBalance`
- `eth_gasPrice`

(See [Unsupported JSON-RPC Methods](#unsupported-json-rpc-methods)).

#### **Example Request**
```json
{
  "jsonrpc": "2.0",
  "method": "eth_blockNumber",
  "params": [],
  "id": 1
}
```
#### **Example Response**
```json
{
  "jsonrpc": "2.0",
  "result": "0xa5b9",
  "id": 1
}
```

---

### **Special Handling: `eth_sendRawTransaction`**
1. Decodes the raw signed transaction.
2. Calls KASPA Wallet for transaction submission to the Base Layer.
3. Returns transaction hash if the previous steps succeed.

#### **Example Request**
```json
{
  "jsonrpc": "2.0",
  "method": "eth_sendRawTransaction",
  "params": ["0xf86b..."], 
  "id": 1
}
```
#### **Example Success Response**
```json
{
  "jsonrpc": "2.0",
  "result": "0xabc123...",
  "id": 1
}
```

#### **Unsupported JSON-RPC Methods**
The **IGRA EL Client** and, thus, the **IGRA RPC Provider** lack the ability to handle signing operations. As a result, the following JSON-RPC methods that require signing functionality are not supported.
- `eth_sign`
- `eth_signTransaction`
- `eth_sendTransaction`

---

## 🔬 Running Tests
We use **`mockito`** for testing API responses.
```sh
cargo test
```

---

## ⚙️ Configuration
The following environment variables can be used:  

| Variable                  | Description                          | Default                          |
|---------------------------|--------------------------------------|----------------------------------|
| `SERVER_HOST`             | Address this app listen requests at  | `127.0.0.1`                      |
| `SERVER_PORT`             | Port this app listen requests at     | `8535`                           |
| `EL_RPC_URL`              | URL of the IGRA EL Client            | `http://127.0.0.1:8545`          |
| `WALLET_COMMAND_TEMPLATE` | Shell command to call KASPA Wallet   | `sh -c 'echo {} >> /tmp/tx_log'` |

Example: Run with a custom node URL.
```sh
EL_RPC_URL="http://igra-el-client:8545" cargo run
```

---

## 🛠 Known Issues and Future Improvements
- BUG: Config ignores the environment variables.
- Interface with KASPA Wallet needs to be improved.

---

## 📜 License
This project is licensed under the MIT License.
