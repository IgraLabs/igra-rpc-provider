use tokio::process::Command;
use shlex;

use crate::config::WalletConfig;

/// Calls the KASPA Wallet to send to KASPA DAG a transaction with the L2 payload.
/// Currently, the IGRA version of the KASPA Wallet supports only a CLI interface.
/// This function triggers a configurable shell command to interact with the Wallet.
///
/// # Parameters:
/// - `raw_tx`: A string slice representing the raw transaction to be processed.
/// - `wallet_config`: Reference to the wallet configuration, containing the shell command template.
///
/// # Returns:
/// - `Ok(())` if the command executes successfully.
/// - `Err(String)` if the command fails.
///
/// # Errors:
/// Returns an error with a string description if command execution fails or if the shell command returns a non-zero exit code.
pub async fn send_transaction(
    raw_tx: &str,
    wallet_config: &WalletConfig,
) -> Result<(), String> {
    // Replace placeholders in the command template ({} -> raw_tx).
    let command_str = wallet_config.command_template.replace("{}", raw_tx);

    // Parse the command string into the base command and arguments using shlex.
    let args = shlex::split(&command_str)
        .ok_or_else(|| "Failed to parse shell command string".to_string())?;

    // Ensure the parsed command is not empty.
    if args.is_empty() {
        return Err("Parsed shell command is empty".to_string());
    }

    // Execute the command using tokio's asynchronous process manager.
    let output = Command::new(&args[0])
        .args(&args[1..])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    // Check the execution result.
    if output.status.success() {
        Ok(())
    } else {
        Err("Shell command failed".to_string())
    }
}
