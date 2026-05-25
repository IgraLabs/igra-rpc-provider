# Entry Transaction CLI

A command-line tool for bridging KAS from L1 (KASPA) to L2 (IGRA) by creating Entry Transactions that lock KAS and mint equivalent iKAS tokens.

> **Post-Toccata KIP-21 enforcement.** Entry transactions carry a real IGRA payload
> and are now subject to the same KIP-21 lane gates as `eth_sendRawTransaction`:
> the daemon-built tx is validated against the configured `IGRA_LANE_ID` before
> mining (catches a `KASWALLET_SUBNETWORK_ID` mismatch early), the payload is
> nonce-mined until the tx id starts with `TX_ID_PREFIX`, then re-validated
> before broadcast. See the [KIP-21 Lane Enforcement section in the main README](../README.md#kip-21-igra-lane-enforcement)
> for the full invariant set.

## Quick Start

```bash
# Build
cargo build --release --bin entry_transaction_sender

# Set environment variables
export WALLET_TO_ADDRESS='kaspatest:qqfrt9vlrpl98m8gwsrw45ynvpgxcrl87x3h2337k8ft4eyhacyqumc0t9vng'
export WALLET_DAEMON_URI='http://localhost:8082'
export KASWALLET_PASSWORD=''  # Empty for no password

# KIP-21 lane — must match the kaswallet daemon's KASWALLET_SUBNETWORK_ID.
# Required for production; set LANE_ENFORCEMENT_DISABLED=true for dev/tests.
export IGRA_LANE_ID='97b10000'
export TX_ID_PREFIX='97b1'

# Send transaction
cargo run --bin entry_transaction_sender -- \
  --recipient kaspatest:qprjv0e4a2l2t56870d6jwkvf9dnjnynhzr0a3kf4spndpz9f6hmxy0ux9yte \
  --amount 25.0 \
  --l2-address 0xb4E3589E55Fef7F4A47090E7f7869c1d2083C7bF
```

## Usage

### Required Arguments

| Flag | Description | Example |
|------|-------------|---------|
| `-r, --recipient` | Kaspa **locking-script** address on L1 where the KAS coins will be locked | `kaspa:q...` |
| `-a, --amount`    | Amount in KAS (supports decimals like 1.5) | `1.5` |
| `-l, --l2-address`| Ethereum address on L2 for iKAS minting | `0x742d35Cc...` |

### Examples

**Send 1 KAS:**
```bash
entry_transaction_sender -r kaspa:qpam... -a 1 -l 0x742d35Cc...
```

**Send 1.5 KAS:**
```bash
entry_transaction_sender -r kaspa:qpam... -a 1.5 -l 0x742d35Cc...
```

**Send 0.00000001 KAS (1 SOMPI):**
```bash
entry_transaction_sender -r kaspa:qpam... -a 0.00000001 -l 0x742d35Cc...
```

## Prerequisites

1. **Configuration**: Ensure `config.toml` has wallet, mining, and lane settings
2. **Wallet**: KASPA wallet daemon must be running, started with
   `KASWALLET_SUBNETWORK_ID` matching the RPC's `IGRA_LANE_ID`
3. **Environment Variables**:
   - `WALLET_TO_ADDRESS`: The source wallet address that holds the KAS to be locked
   - `WALLET_DAEMON_URI`: URI of the wallet daemon (e.g., `http://localhost:8082`)
   - `KASWALLET_PASSWORD`: Wallet password (can be empty string for no password)
   - `IGRA_LANE_ID`: 4-byte KIP-21 lane namespace (e.g. `97b10000`).
     **Required for production**; the CLI refuses to start without it
     unless `LANE_ENFORCEMENT_DISABLED=true` is explicitly set (dev only).
   - `TX_ID_PREFIX` *(optional, default `97b1`)*: Required prefix the
     mined tx id must start with. Set to whatever the lane convention
     dictates.
   - `LANE_ENFORCEMENT_DISABLED` *(optional, default `false`)*: Dev/test
     escape hatch — when `true` **and** `IGRA_LANE_ID` is unset, runs
     without KIP-21 enforcement (logs a loud warning). Never use in
     production.

## Output

**Success:** the broadcast tx id will start with the configured `TX_ID_PREFIX`
(the mining loop guarantees it; the pre-broadcast lane gate re-verifies it):

```
✅ Entry transaction sent successfully!
   Transaction ID: 97b1a2b3c4d5...
   Recipient: kaspa:qpam...
   Amount: 1.50000000 KAS (150000000 SOMPI)
   L2 Address: 0x742d35Cc...
   Processing time: 682.875667ms
```

Expect server logs along the way (with default `info` level):

```
INFO mine_transaction: Mining operation started ...
INFO mining_operations: Successfully mined transaction ...
   mining_nonces_tried=... mining_duration_ms=... transaction_final_id=97b1...
INFO lane_enforcement: lane PreBroadcast validation passed
INFO clients::wallet_caller: Transaction broadcast successfully! ...
```

**Errors:**
- Exit code 1: Invalid inputs (addresses, amounts)
- Exit code 2: Configuration/connection issues — including missing
  `IGRA_LANE_ID` without the explicit `LANE_ENFORCEMENT_DISABLED=true`
  escape hatch.
- Exit code 3: Transaction/mining failures, including the four KIP-21
  invariants (`-32016 LaneEnforcementFailed: version|subnetwork|payload|prefix`).

## Common Issues

| Problem | Solution |
|---------|----------|
| "Wallet password not set" | `export KASWALLET_PASSWORD=""` (empty for no password) |
| "Wallet address not set" | `export WALLET_TO_ADDRESS='kaspatest:qqfrt9...'` |
| "Invalid Kaspa address" | Use format: `kaspa:qpam...` or `kaspatest:qprjv...` |
| "Invalid Ethereum address" | Use 40 hex chars: `0x742d35Cc...` |
| "Invalid amount" | Use valid KAS amount: `1`, `1.5`, `0.00000001` |
| "Connection failed" | Check wallet daemon is running. Set: `export WALLET_DAEMON_URI="http://localhost:8082"` |
| "Mining timeout" | Increase `mining.timeout_seconds` in `config.toml` (1-300) |
| `Configuration error: Lane config: IGRA_LANE_ID is required` | Set `export IGRA_LANE_ID='97b10000'` (matching the daemon's `KASWALLET_SUBNETWORK_ID`), or, for dev/tests only, `export LANE_ENFORCEMENT_DISABLED=true`. |
| `LaneEnforcementFailed: pre-mining: subnetwork` | The daemon emits a different subnetwork than the RPC expects. Check the daemon's `KASWALLET_SUBNETWORK_ID` matches `IGRA_LANE_ID` (4-byte hex, no `0x`). The server log contains the actual and expected values. |
| `LaneEnforcementFailed: pre-mining: version` | The daemon emits a v0 (pre-Toccata) tx, but lane enforcement requires v1. Update the wallet daemon to a Toccata-aware build. |
| `LaneEnforcementFailed: pre-broadcast: prefix` | Mining produced a tx whose id does not start with `TX_ID_PREFIX`. Usually a bug — re-run; if persistent, file a ticket with the server log. |
| `MiningTimeout` (`-32007`) | Prefix is too long for the configured timeout. Either shorten the prefix or raise `mining.timeout_seconds`. |

## Amount Format

The CLI accepts amounts in **KAS** with decimal support:
- `1` = 1 KAS = 100,000,000 SOMPI
- `1.5` = 1.5 KAS = 150,000,000 SOMPI
- `0.00000001` = 1 SOMPI (smallest unit)

## Automation

```bash
#!/bin/bash
# Set environment variables
export WALLET_TO_ADDRESS='kaspatest:qqfrt9vlrpl98m8gwsrw45ynvpgxcrl87x3h2337k8ft4eyhacyqumc0t9vng'
export WALLET_DAEMON_URI='http://localhost:8082'
export KASWALLET_PASSWORD=''  # Empty for no password

# KIP-21 lane — must match the daemon's KASWALLET_SUBNETWORK_ID.
export IGRA_LANE_ID='97b10000'
export TX_ID_PREFIX='97b1'

# Check exit code for success/failure
if cargo run --bin entry_transaction_sender -- -r "$RECIPIENT" -a "$AMOUNT" -l "$L2_ADDR"; then
    echo "Success!"
else
    echo "Failed with exit code $?"
fi

# Example with variables
RECIPIENT="kaspatest:qprjv0e4a2l2t56870d6jwkvf9dnjnynhzr0a3kf4spndpz9f6hmxy0ux9yte"
AMOUNT="25.0"  # 25 KAS
L2_ADDR="0xb4E3589E55Fef7F4A47090E7f7869c1d2083C7bF"

cargo run --bin entry_transaction_sender -- -r "$RECIPIENT" -a "$AMOUNT" -l "$L2_ADDR"
```

## Disabling Lane Enforcement (dev / pre-Toccata only)

For local development against a pre-Toccata network, or for CI tests
without a real wallet daemon, set the explicit escape hatch:

```bash
export LANE_ENFORCEMENT_DISABLED=true
unset IGRA_LANE_ID                  # or leave it unset

cargo run --bin entry_transaction_sender -- \
  --recipient kaspa:qpam... --amount 1 --l2-address 0x...
```

The CLI will log a loud `warn!` at startup
(`KIP-21 LANE ENFORCEMENT DISABLED via LANE_ENFORCEMENT_DISABLED=true`)
and behave as it did pre-Toccata: mining still runs (to satisfy
`TX_ID_PREFIX`), but the subnetwork / version / payload-length gates
are skipped. **Never set this in production.**