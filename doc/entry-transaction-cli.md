
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
| "Connection failed" | Check wallet daemon is running. For Docker: `export WALLET_DAEMON_URI="http://localhost:8082"` |
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
# Set environment variables
export KASWALLET_PASSWORD="your-password"
export WALLET_DAEMON_URI="http://localhost:8082"  # If using Docker

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