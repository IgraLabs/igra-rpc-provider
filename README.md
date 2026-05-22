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

### Breaking Changes

**Post-Toccata**: Lane binding is now first-class on the consensus
side via `Transaction.subnetwork_id`; the previous pre-Toccata
selection mechanism (matching a transaction-id prefix in the payload)
has been removed entirely. Set `IGRA_LANE_ID` (or the `[igra]` section
in `config.toml`) instead, and make sure the connected kaswallet
daemon is configured with the matching `KASWALLET_SUBNETWORK_ID`.

To migrate:
- Remove the legacy mining-related section from `config.toml` and any
  related environment variables (the old prefix and mining-timeout
  knobs are gone).
- Add `[igra] lane_id = "97b10000"` (or whatever 4-byte namespace your
  deployment uses) to `config.toml`, or set `IGRA_LANE_ID=97b10000`.

### Environment Variables

| Variable                | Description                              | Default                          |
|-------------------------|------------------------------------------|----------------------------------|
| `SERVER_HOST`           | Address this app listen requests at      | `127.0.0.1`                      |
| `SERVER_PORT`           | Port this app listen requests at         | `8535`                           |
| `EL_URL`                | URL of the IGRA EL Client                | `http://127.0.0.1:8545`          |
| `WALLET_DAEMON_URI`     | URI of the Kaspa Wallet daemon           | -                                |
| `READ_ONLY`             | Enable read-only mode (blocks writes)    | `false`                          |
| `IGRA_LANE_ID`          | 4-byte SubnetworkId namespace as exactly 8 lowercase hex chars, no `0x` (e.g. `97b10000`). RPC zero-pads to the full 20-byte SubnetworkId per KIP-21. | required (from `config.toml`) |
| `EL_WS_URL`             | WebSocket URL of the IGRA EL Client      | Derived from `EL_URL` (ws://, port 8546) |

### Deployment

`IGRA_LANE_ID` must match the connected kaswallet daemon's
`KASWALLET_SUBNETWORK_ID`. Both take the same 4-byte namespace string
(e.g. `97b10000`). On a mismatch the RPC validates the unsigned tx
returned by the daemon, rejects it with `LaneValidationFailed` before
signing, and the user sees an `eth_sendRawTransaction` failure with an
explanatory error message — no tx is broadcast.

#### Lane alignment check

RPC has no startup probe for the daemon's configured lane yet (kaswallet
exposes no `GetSubnetworkId` RPC at the time of writing — see the PRD's
follow-ups for the planned RPC addition), so a lane mismatch is only
caught on the first transaction. To verify alignment *before* the first
submission, grep both processes' startup logs and compare the
namespaces:

```sh
# RPC side — look for the dedicated banner emitted at startup.
journalctl -u igra-rpc-provider --since=-5m \
  | grep 'igra_lane_namespace_4b' | tail -1

# kaswallet side — the daemon logs its active subnetwork at startup.
journalctl -u kaswallet --since=-5m \
  | grep 'subnetwork_id' | tail -1
```

The two 4-byte namespaces must match exactly. The RPC banner reads:

```
IGRA lane configured — verify the connected kaswallet daemon was
started with matching KASWALLET_SUBNETWORK_ID …
igra_lane_namespace_4b=97b10000  igra_lane_id_20b=97b1000000000000000000000000000000000000
```

In containerised deployments where startup logs roll fast, set both
variables from a single shared env file (or the same Kubernetes
ConfigMap key) so they cannot drift.

Example: Run with a custom node URL.
```sh
EL_URL="http://igra-el-client:8545" cargo run
```

Example: Run in read-only mode (blocks all write operations).
```sh
READ_ONLY=true cargo run
```

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

The `entry_transaction_sender` binary can also be run via Docker. Make sure to pass all required environment variables explicitly:

```sh
# Set environment variables in your shell
export WALLET_TO_ADDRESS='kaspa:qpt9...'
export WALLET_DAEMON_URI='http://kaswallet:8082'
export KASWALLET_PASSWORD=''
export IGRA_LANE_ID='97b10000'

# Run entry_transaction_sender - each -e flag passes the variable to the container
docker run --rm \
  -e WALLET_TO_ADDRESS \
  -e WALLET_DAEMON_URI \
  -e KASWALLET_PASSWORD \
  -e IGRA_LANE_ID \
  --network your-network \
  --entrypoint /app/entry_transaction_sender \
  igranetwork/rpc-provider:latest \
  --recipient kaspa:qpv5... \
  --amount 100 \
  --l2-address 0xd850cc8fdd0348f12df47fd597784007c3c05f75
```

**Common mistake**: If you set `IGRA_LANE_ID=97b10000` in your shell but
omit `-e IGRA_LANE_ID` from the docker command, the container will fall
back to the value in `config.toml` (or fail to start if neither is set).
The configured value must match the connected kaswallet daemon's
`KASWALLET_SUBNETWORK_ID`.

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
