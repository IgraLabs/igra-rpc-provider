# Multi-wallet parallelism (Kaswallet fan-out) and batch RPC concurrency

## Problem statement

This repo is an HTTPS JSON-RPC provider for L2 wallets:
- Most methods are forwarded to the EL (`ProxyService`).
- `eth_sendRawTransaction` is special: it must be embedded in a Kaspa L1 tx via **Kaswallet**, mined (prefix constraint), signed, and broadcast.

The reported performance issue was consistent with two sources of serialization:
1) **Batch JSON-RPC** was handled one-by-one, even though requests were independent.
2) **Transaction submission** (`eth_sendRawTransaction`) was effectively single-lane:
   - One global queue.
   - One background worker consuming it.
   - One Kaswallet daemon configured, so all submissions were bottlenecked behind a single external dependency.

Goal: allow **one provider → many kaswallet daemons** so submissions can run in parallel across wallets, while keeping *per-wallet* work sequential (wallet UTXO state / gRPC client safety).

---

## Summary of changes

### Change map (files touched)

- `src/api/rpc.rs`: batch execution is parallelized with `JoinSet` (order preserved): `src/api/rpc.rs#L22`
- `src/config/app.rs`: added `wallets: Vec<WalletConfig>` + validation: `src/config/app.rs#L15`, `src/config/app.rs#L125`
- `src/main.rs`: connect to N wallet daemons and start the worker pool: `src/main.rs#L53`, `src/main.rs#L75`
- `src/services/transaction.rs`:
  - new `WalletBackend` type: `src/services/transaction.rs#L71`
  - `start_transaction_processor` now spawns one worker per backend: `src/services/transaction.rs#L97`
  - `TransactionRequest` now uses a `oneshot` response channel: `src/services/transaction.rs#L89`
  - `process_wallet_call` now targets a specific backend (no global wallet caller): `src/services/transaction.rs#L591`
- `src/lib.rs`: removed global `wallet_caller` from `AppState`: `src/lib.rs#L19`
- `config.toml`: documented `[[wallets]]` format: `config.toml#L12`
- Tests:
  - `tests/config_wallets.rs`
  - `tests/multi_wallet_processing.rs`

### 1) Batch JSON-RPC is now parallel (order preserved)

**File:** `src/api/rpc.rs`

Old behavior: batch requests were drained and `await`ed in a loop (serial).

New behavior:
- Spawn one task per request via `tokio::task::JoinSet`.
- Store results into a pre-sized vector by original index to preserve JSON-RPC ordering.

Key code:
- `handle_rpc` batch branch: `src/api/rpc.rs#L22`

Why: batch calls are usually “read-only” and independent; serial handling artificially increases latency.

---

### 2) Config supports multiple wallets

**File:** `src/config/app.rs`

New field:
- `AppConfig.wallets: Vec<WalletConfig>` (optional, defaults to empty)

Key code:
- `AppConfig` struct: `src/config/app.rs#L15`
- Validation of `wallets`: `src/config/app.rs#L125`

Why: this is the minimal config change that enables N wallet backends without breaking the existing single-wallet config (`[wallet]` stays supported).

**File:** `config.toml`

New example format:
- Primary wallet: `[wallet] ...`
- Additional wallets: `[[wallets]] ...` (one per daemon)

Key snippet:
- `config.toml#L8`

---

### 3) Transaction processor is now a worker pool (one worker per wallet backend)

**File:** `src/services/transaction.rs`

Old behavior:
- `start_transaction_processor(config)` spawned **one** background worker that processed the queue sequentially.

New behavior:
- Introduced `WalletBackend` (a connected Kaswallet client):
  - Holds a `WalletCaller` instance per wallet daemon URI.
- `start_transaction_processor(config, wallet_backends, gas_price_service)` now spawns:
  - **one `transaction_worker` per wallet backend**
- All workers share one queue receiver, but once a worker takes a job it runs independently.
  - `GasPriceService` is passed in so all workers share the same 1-second base-fee cache.

Key code:
- `WalletBackend`: `src/services/transaction.rs#L71`
- `start_transaction_processor` (new signature + worker spawn): `src/services/transaction.rs#L97`
- `transaction_worker` loop: `src/services/transaction.rs#L124`
 - `TransactionRequest` uses a `oneshot::Sender` to return completion to the HTTP handler: `src/services/transaction.rs#L89`
 - `process_wallet_call` is backend-scoped (worker passes `&WalletBackend`): `src/services/transaction.rs#L227`, `src/services/transaction.rs#L591`

Why this design:
- Each wallet daemon has its own UTXO set / internal state; spreading load across multiple daemons can increase throughput.
- Within a single wallet backend we still serialize wallet RPC usage:
  - `WalletCaller` uses an internal `Mutex<WalletClient<Channel>>` so gRPC calls are sequential per wallet.
- Parallelism happens **across** wallet backends (N daemons → N workers).

---

### 4) `AppState` no longer stores a single wallet caller

**File:** `src/lib.rs`

Old behavior:
- `AppState` stored `wallet_caller: Arc<WalletCaller>` which forced a single wallet backend.

New behavior:
- `AppState` only stores the transaction queue sender and proxy service.
- Wallet selection happens entirely inside the background worker pool.

Key code:
- `AppState` definition: `src/lib.rs#L19`

Why: keeping wallet backends inside the processor avoids adding selection logic into API handlers and keeps the “fan-out” concern localized.

---

### 5) Startup connects to N wallets and starts the pool

**File:** `src/main.rs`

New behavior:
- Build list of wallet configs:
  - Always include `config.wallet`
  - Append `config.wallets`
- Attempt to connect all wallet backends; start only with the reachable ones.
- Refuse to start if **zero** wallet backends are reachable.

Key code:
- Wallet config fan-in + connect loop: `src/main.rs#L53`
- Start processor with pool: `src/main.rs#L75`

Why: graceful boot with partial wallet availability, but fail-fast if nothing can process transactions.

---

## How request flow works now

### `eth_sendRawTransaction`

1) API validates and queues the transaction:
   - `transaction::process_transaction` enqueues a `TransactionRequest` and waits on a `oneshot` for completion.
   - `src/services/transaction.rs#L286`

   Note: if queueing fails (channel closed / backpressure), we now return a JSON-RPC error instead of returning a hash that will never be processed.
   - queue send + error response: `src/services/transaction.rs#L360`

2) One of the wallet workers picks the request:
   - Computes effective base fee (cached) and validates tx fees.
   - Builds IGRA payload and calls the assigned wallet backend.
   - `src/services/transaction.rs#L124`

3) Wallet backend performs the full flow:
   - Create unsigned tx (Kaswallet)
   - Mine payload (prefix constraint)
   - Sign (Kaswallet)
   - Broadcast (Kaswallet)

Parallelism:
- Multiple transactions can be processed simultaneously if there are multiple wallet backends.
- Within a single backend, the wallet gRPC client is used sequentially.

---

## Tests added (prove multi-wallet works)

### 1) Config parsing for `[[wallets]]`

**File:** `tests/config_wallets.rs`

Verifies:
- TOML deserializes into `AppConfig.wallets`.
- Each wallet entry validates via `WalletConfig::validate`.

---

### 2) End-to-end multi-wallet concurrency using real gRPC

**File:** `tests/multi_wallet_processing.rs`

This is a functional proof that:
- The transaction processor uses **multiple wallet backends**.
- Two transactions can reach the *broadcast* step concurrently.

How the test works:
- Starts a mock EL HTTP server (wiremock) to satisfy base-fee fetching.
- Starts **two in-process gRPC Wallet servers** (tonic) implementing the Kaswallet proto.
- Starts the transaction processor with **two** `WalletBackend`s.
- Submits two real signed Ethereum tx payloads concurrently.
- Uses a shared “broadcast gate” that only lets `broadcast()` return once **two** broadcasts are in-flight, proving parallelism across wallets.
- Asserts each wallet daemon received exactly one `CreateUnsignedTransactions`, `Sign`, and `Broadcast` call.

Run:
```bash
cargo test -p igra-rpc-provider --test multi_wallet_processing
```

---

## Operational notes / limitations

- Worker selection is “first available” (not strict round-robin). Under uneven wallet latency, faster wallets will naturally take more work.
- The queue is still single, but workers parallelize processing once they’ve received a job.
- The wallet password is still sourced from the `KASWALLET_PASSWORD` env var (required for all wallet daemons).
