# Entry Transaction CLI

A command-line tool for bridging KAS from L1 (KASPA) to L2 (IGRA) by creating Entry Transactions that lock KAS and mint equivalent iKAS tokens.

## Quick Start

```bash
# Build
cargo build --release --bin entry_transaction_sender

# Set wallet password
export KASWALLET_PASSWORD="your-password"

# Send transaction
entry_transaction_sender \
  --recipient kaspa:qpam... \
  --amount 1.5 \
  --l2-address 0x742d35Cc...
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

1. **Configuration**: Ensure `config.toml` has wallet and mining settings
2. **Wallet**: KASPA wallet daemon must be running
3. **Password**: Set `KASWALLET_PASSWORD` environment variable

## Output

**Success:**
```
✅ Entry transaction sent successfully!
   Transaction ID: 97b1a2b3c4d5...
   Recipient: kaspa:qpam...
   Amount: 1.50000000 KAS (150000000 SOMPI)
   L2 Address: 0x742d35Cc...
   Processing time: 682.875667ms
```

**Errors:**
- Exit code 1: Invalid inputs (addresses, amounts)
- Exit code 2: Configuration/connection issues
- Exit code 3: Transaction/mining failures

## Common Issues

| Problem | Solution |
|---------|----------|
| "Wallet password not set" | `export KASWALLET_PASSWORD="..."` |
| "Invalid Kaspa address" | Use format: `kaspa:qpam...` |
| "Invalid Ethereum address" | Use 40 hex chars: `0x742d35Cc...` |
| "Invalid amount" | Use valid KAS amount: `1`, `1.5`, `0.00000001` |
| "Connection failed" | Check wallet daemon is running |
| "Mining timeout" | Increase timeout in `config.toml` |

## Amount Format

The CLI accepts amounts in **KAS** with decimal support:
- `1` = 1 KAS = 100,000,000 SOMPI
- `1.5` = 1.5 KAS = 150,000,000 SOMPI
- `0.00000001` = 1 SOMPI (smallest unit)

This eliminates the need to manually count zeros and prevents expensive mistakes.

## Automation

```bash
#!/bin/bash
# Check exit code for success/failure
if entry_transaction_sender -r "$RECIPIENT" -a "$AMOUNT" -l "$L2_ADDR"; then
    echo "Success!"
else
    echo "Failed with exit code $?"
fi

# Example with variables
RECIPIENT="kaspa:qpam..."
AMOUNT="1.5"  # 1.5 KAS
L2_ADDR="0x742d35Cc..."

entry_transaction_sender -r "$RECIPIENT" -a "$AMOUNT" -l "$L2_ADDR"
```
