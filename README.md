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

## Architecture Overview

The IGRA RPC Provider is built using **Domain-Driven Design (DDD)** principles with **Single Responsibility Principle (SRP)** to ensure maintainability and testability. The architecture consists of three main layers:

### 🏗️ Layered Architecture
- **API Layer**: Handles HTTP requests and JSON-RPC protocol concerns
- **Service Layer**: Contains business logic with domain-specific services
- **Client Layer**: Manages external service communications (EL, Wallet, etc.)

### 🔧 Core Services
- **Transaction Processor**: Handles Ethereum transaction validation and processing
- **Gas Manager**: Manages gas price calculation and EIP-1559 validation
- **Proxy Service**: Forwards requests to the Execution Layer
- **Wallet Service**: Abstracts Kaspa wallet operations and communication

### 📋 Configuration Management
Domain-specific configuration modules with comprehensive validation:
- Server, Gas, Wallet, Proxy, Security, and Mining configurations
- Runtime validation and structured error handling
- Backward compatibility with existing configuration formats

### 🔍 For Detailed Architecture Information
See [Architecture Documentation](doc/architecture.md) for comprehensive diagrams, service interactions, design decisions, and implementation details.

### 📈 Tx Performance CLI
See `tx_perf` usage and examples in [doc/tx-perf-cli.md](doc/tx-perf-cli.md).

## Features
✅ **Proxy Mode**: Transparently forwards "read-only" JSON-RPC requests (`eth_blockNumber`, `eth_getBalance`, etc.) to the IGRA EL client.
✅ **Custom Handling for `eth_sendRawTransaction`**:
  - Calls the KASPA Wallet for transaction submission to the Base Layer (KASPA DAG).
  - Returns transaction hash only if all checks pass and the transaction gets submitted.
✅ **Read-Only Mode**: When enabled via `READ_ONLY=true`, blocks all write operations (`eth_sendRawTransaction`, `personal_*`, `admin_*`).
✅ **WebSocket Support**: Full WebSocket proxy on `GET /` — subscriptions (`eth_subscribe`/`eth_unsubscribe`) relay through reth WS, all other methods use the same routing as HTTP (including `eth_sendRawTransaction` → L1 pipeline and gas price floor).
✅ **Health Endpoint**: `GET /health` verifies EL connectivity for load balancer health checks.
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
By default, the server listens on **`127.0.0.1:8535`**.

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

Submissions follow Ethereum **mempool-accept** semantics: the request is validated
synchronously and the transaction hash is returned as soon as the transaction is accepted
into the processing queue.

1. Decodes the raw signed transaction and validates it synchronously (RLP/format, then the
   EIP-1559/legacy gas-fee floor against the effective base fee).
2. Enqueues the transaction and **returns the transaction hash immediately** (before
   Base-Layer submission).
3. Mining, signing, and L1 broadcast to the KASPA DAG happen **asynchronously** in a
   background worker. Failures after acceptance are logged/alerted (see below), not returned
   in the RPC response.

Clients should reconcile by transaction hash — poll `eth_getTransactionReceipt` for inclusion
rather than treating the `eth_sendRawTransaction` response as confirmation of broadcast.

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

## 🧰 JSON-RPC Error Codes

App-specific errors returned by `eth_sendRawTransaction` and related write
paths. Codes outside this table are passed through from the upstream EL
client unmodified.

> **Mempool-accept semantics:** only **synchronous accept-path** errors reach the client —
> request/format errors (`-32001`/`-32602`), the gas-fee floor (`-32602`,
> `INSUFFICIENT_GAS_FEE`), base-fee-fetch failure (`-32000`, `BASE_FETCH_FAILED`, retryable),
> and a full queue (`-32000`, `QUEUE_FULL`, retryable). Failures that occur **after** the hash
> is returned — mining, wallet/UTXO, signing, L1 broadcast (`-32005`–`-32011`, `-32014`/`-32015`),
> and KIP-21 lane enforcement (`-32016`, `LaneEnforcementFailed`) — are emitted as structured
> `transaction_alerts` logs, **not** RPC errors on the `eth_sendRawTransaction` response, and must
> be reconciled by transaction hash. (The `entry_transaction_sender` CLI submits synchronously and
> still returns these codes directly.)

| Code     | Symbol                        | Cause / Operator action                                                                                          |
|----------|-------------------------------|------------------------------------------------------------------------------------------------------------------|
| `-32000` | Generic server error          | `ConfigError`, `ElCallError`, `JsonRpcError`, `Internal`, `ReadOnlyMode` — see message for the specific subclass. |
| `-32001` | `InvalidTransactionFormat`    | Raw EVM tx failed RLP decoding or invariant checks.                                                              |
| `-32002` | `MethodNotAllowed`            | The requested JSON-RPC method is on the deny-list (e.g. `eth_sign`, `eth_sendTransaction`).                      |
| `-32003` | `InvalidPayload`              | IGRA payload structure rejected before submission to the wallet.                                                 |
| `-32004` | `SerializationError`          | Internal serialization failure constructing the L2 payload.                                                      |
| `-32005` | `WalletCallError`             | Generic failure calling the kaswallet daemon.                                                                    |
| `-32006` | `MiningError`                 | Generic mining failure (rare; specific cases use -32007/-32008/-32010/-32011).                                   |
| `-32007` | `MiningTimeout`               | Nonce mining exceeded `MINING_TIMEOUT_SECONDS`. Lower difficulty or raise the timeout.                           |
| `-32008` | `NonceExhaustion`             | All `2^32` nonces tried without matching `TX_ID_PREFIX`. Prefix may be too long for the configured timeout.       |
| `-32009` | `TransactionCodecError`       | proto↔kaspa encode/decode failed; usually a daemon/RPC version drift.                                            |
| `-32010` | `MiningConfigError`           | Mining config rejected at startup.                                                                               |
| `-32011` | `MiningInvalidState`          | Mining hit an invalid transaction state.                                                                         |
| `-32012` | `WalletError`                 | Wallet daemon returned a structured error.                                                                       |
| `-32014` | `UtxoExhausted`               | No funds available to send. Top up the wallet.                                                                   |
| `-32015` | `RetryExhausted`              | Wallet daemon retry budget exceeded (typically chained UTXO exhaustion).                                         |
| `-32016` | `LaneEnforcementFailed`       | KIP-21 invariant violated. Message names the failing class (`version`/`subnetwork`/`payload`/`prefix`). Full diagnostic — actual lane, expected lane, tx id — is in the server log at `warn!` level. The most common cause is `KASWALLET_SUBNETWORK_ID` not matching `IGRA_LANE_ID`. |

---

## ⚙️ Configuration

### Breaking Changes

**v0.4.0** (post-Toccata): KIP-21 lane enforcement is now **required by
default**. Startup fails unless one of the following is set:
- `IGRA_LANE_ID=97b10000` (or your deployment's lane namespace), **or**
- `LANE_ENFORCEMENT_DISABLED=true` (dev/test only; logs a loud warning).

The kaswallet daemon must be configured with a matching
`KASWALLET_SUBNETWORK_ID`. See the [KIP-21 IGRA Lane Enforcement](#kip-21-igra-lane-enforcement)
section below.

**v0.3.0**: The environment variable `MINING_REQUIRED_PREFIX` has been renamed to `TX_ID_PREFIX` and the config field `mining.required_prefix` is now `mining.tx_id_prefix`.

To migrate:
- Environment variables: `MINING_REQUIRED_PREFIX` → `TX_ID_PREFIX`
- config.toml: `[mining] required_prefix` → `[mining] tx_id_prefix`

### Environment Variables

| Variable                | Description                              | Default                          |
|-------------------------|------------------------------------------|----------------------------------|
| `SERVER_HOST`           | Address this app listen requests at      | `127.0.0.1`                      |
| `SERVER_PORT`           | Port this app listen requests at         | `8535`                           |
| `EL_URL`                | URL of the IGRA EL Client                | `http://127.0.0.1:8545`          |
| `WALLET_DAEMON_URI`     | URI of the Kaspa Wallet daemon           | -                                |
| `READ_ONLY`             | Enable read-only mode (blocks writes)    | `false`                          |
| `TX_ID_PREFIX`          | Required prefix for mined transaction IDs (hex string, e.g., "97b1" or "0x97b1")| `97b1`                   |
| `EL_WS_URL`             | WebSocket URL of the IGRA EL Client      | Derived from `EL_URL` (ws://, port 8546) |
| `MINING_TIMEOUT_SECONDS`| Mining timeout in seconds (1-300)        | `10`                             |
| `IGRA_LANE_ID`          | KIP-21 IGRA lane id as 4-byte namespace (8 lowercase hex chars, no `0x`, e.g. `97b10000`). **Required for production** — startup fails without it unless `LANE_ENFORCEMENT_DISABLED=true`. | _unset_                          |
| `LANE_ENFORCEMENT_DISABLED` | Dev/test escape hatch: when `true` **and** `IGRA_LANE_ID` is unset, the RPC starts without KIP-21 lane enforcement (legacy native-subnetwork behavior) and logs a loud warning. Do not set in production. | `false` |

Example: Run with a custom node URL.
```sh
EL_URL="http://igra-el-client:8545" cargo run
```

Example: Run in read-only mode (blocks all write operations).
```sh
READ_ONLY=true cargo run
```

### KIP-21 IGRA Lane Enforcement

Post-Toccata, every `eth_sendRawTransaction` payload-carrying tx must satisfy
four KIP-21 invariants on broadcast:

1. `tx.version >= TX_VERSION_TOCCATA` (v1).
2. `tx.subnetwork_id` equals the configured IGRA lane.
3. `tx.payload` is larger than the 4-byte nonce slot used by mining.
4. The final tx id starts with the configured `TX_ID_PREFIX` (enforced via
   the existing nonce-mining loop).

Lane enforcement is **required by default**. Set `IGRA_LANE_ID` to the
4-byte lane namespace (e.g. `97b10000`) **and** configure the kaswallet
daemon with a matching `KASWALLET_SUBNETWORK_ID`. The kaswallet daemon
owns lane construction (setting `subnetwork_id`, picking
`TX_VERSION_TOCCATA = 1`, and emitting v1 input mass commitments); the
RPC provider validates the daemon-built tx twice — once before mining
(catches config mismatch early) and once before broadcast (enforces all
four invariants including the prefix).

```sh
# Wallet daemon — see kaswallet docs for the full command
KASWALLET_SUBNETWORK_ID=97b10000 kaswallet-daemon ...

# RPC provider
IGRA_LANE_ID=97b10000 TX_ID_PREFIX=97b1 WALLET_DAEMON_URI=... cargo run
```

**Dev / test only** — to start the RPC without lane enforcement (e.g.
against a pre-Toccata network or for local testing without a daemon),
set `LANE_ENFORCEMENT_DISABLED=true`. The RPC will start with a loud
warning in the log and behave as it did pre-Toccata. **Do not use this
flag in production.**

Mismatched daemon/RPC configuration is detected when the background worker
processes a transaction. Because `eth_sendRawTransaction` now returns the hash
on mempool-accept (before submission), a lane mismatch is **not** returned as a
synchronous `-32016` on the request — it is emitted as a `transaction_alerts`
log (`KIP-21 lane enforcement failed: ...`, severity `critical`) and the
transaction is not broadcast; reconcile by transaction hash (the receipt never
appears). The operator log contains the full diagnostic (actual lane, expected
lane, tx id, env-var hint). (The `entry_transaction_sender` CLI submits
synchronously and still returns `-32016` directly.)

---

## 🐳 Building and Running with Docker

You can build and run the **IGRA RPC Provider** using Docker. This method allows you to avoid installing Rust or any dependencies manually on your system.

### **1️⃣ Build the Docker Image**

To build the Docker image, use the following command in the root directory of the project (where the `Dockerfile` is located):

```sh
docker build -t igra-rpc-provider .
```

This will create a Docker image named `igra-rpc-provider`.

---

### **2️⃣ Run the Application in a Docker Container**

Once the image is built, you can start the application by running a container using the command:

```sh
docker run --name igra-rpc -d -p 8535:8535 igra-rpc-provider
```

This will bind the container's port `8535` (the default port for the server) to your local machine's port `8535`. You can now interact with the JSON-RPC server at `http://127.0.0.1:8535`.

---

### **3️⃣ Environment Configuration**

If your application relies on specific environment variables or external configuration files, you can pass them to the container using the `-e` or `-v` flags, or with the `--env-file` option.

**Important**: Environment variables set in your shell are NOT automatically passed to Docker containers. You must explicitly pass each variable using the `-e` flag.

#### Passing Individual Environment Variables

```sh
docker run -p 8535:8535 \
  -e EL_URL="http://igra-el-client:8545" \
  -e WALLET_DAEMON_URI="http://kaswallet:8082" \
  -e WALLET_TO_ADDRESS="kaspa:qpam..." \
  -e TX_ID_PREFIX="97b2" \
  -e IGRA_LANE_ID="97b10000" \
  --network your-network \
  igra-rpc-provider
```

#### Using an Environment File

```sh
docker run -p 8535:8535 --env-file /path/to/custom.env igra-rpc-provider
```

Ensure that all required dependencies, such as the IGRA EL Client and the KASPA Wallet, are properly configured and accessible to the containerized application.

---

### **4️⃣ Entry Transaction Sender (Docker)**

The `entry_transaction_sender` binary can also be run via Docker. Entry txs
are subject to the same [KIP-21 lane enforcement](#kip-21-igra-lane-enforcement)
as `eth_sendRawTransaction` — `IGRA_LANE_ID` is required and must match
the daemon's `KASWALLET_SUBNETWORK_ID`. Make sure to pass all required
environment variables explicitly:

```sh
# Set environment variables in your shell
export WALLET_TO_ADDRESS='kaspa:qpt9...'
export WALLET_DAEMON_URI='http://kaswallet:8082'
export KASWALLET_PASSWORD=''
export TX_ID_PREFIX='97b1'
export IGRA_LANE_ID='97b10000'        # must match the daemon's KASWALLET_SUBNETWORK_ID

# Run entry_transaction_sender - each -e flag passes the variable to the container
docker run --rm \
  -e WALLET_TO_ADDRESS \
  -e WALLET_DAEMON_URI \
  -e KASWALLET_PASSWORD \
  -e TX_ID_PREFIX \
  -e IGRA_LANE_ID \
  --network your-network \
  --entrypoint /app/entry_transaction_sender \
  igranetwork/rpc-provider:latest \
  --recipient kaspa:qpv5... \
  --amount 100 \
  --l2-address 0xd850cc8fdd0348f12df47fd597784007c3c05f75
```

**Common mistakes:**

- Omitting `-e IGRA_LANE_ID` from the docker command (even if exported in
  your shell) causes the CLI to refuse to start with
  `Configuration error: Lane config: IGRA_LANE_ID is required`.
  Environment variables set in your shell are **not** automatically
  passed to Docker containers — each one needs an explicit `-e` flag.
- `IGRA_LANE_ID` not matching the daemon's `KASWALLET_SUBNETWORK_ID`
  produces `-32016 "pre-mining: subnetwork"` on the first request;
  the server log names both actual and expected values.
- Setting `TX_ID_PREFIX=97b2` in your shell but omitting
  `-e TX_ID_PREFIX` from the docker command leaves the container on the
  default (`97b1`).

For dev or pre-Toccata environments where you want to run the CLI
without lane enforcement, add `-e LANE_ENFORCEMENT_DISABLED=true` and
omit `IGRA_LANE_ID`. The container will log a loud warning at startup.
Never use this flag in production.

---

### **5️⃣ Verify the Service**

Once the container is running, you can verify it using the health endpoint:

```sh
curl http://127.0.0.1:8535/health
```

You should receive a JSON response indicating the service is healthy:
```json
{
  "status": "healthy",
  "block_number": "0xa5b9"
}
```

Alternatively, you can test with a JSON-RPC method like `eth_blockNumber`:

```sh
curl -X POST http://127.0.0.1:8535 \
-H "Content-Type: application/json" \
-d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

You should receive a JSON response containing the block number.
```json
{
  "jsonrpc": "2.0",
  "result": "0xa5b9",
  "id": 1
}
```

---

## 🛠 Known Issues and Future Improvements
- `wss://` protocol is not yet supported for the upstream reth WebSocket connection (only `ws://` for local/Docker reth connections).
- Interface with KASPA Wallet needs to be improved.

---

## 📜 License
This project is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
