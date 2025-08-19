use clap::Parser;
use reqwest::Client;
use serde_json::json;
use tracing::{debug, info};

/// CLI utility to send raw transactions to the EL RPC endpoint
#[derive(Parser, Debug)]
#[command(
    author = "Igra Labs",
    version = "1.0",
    about = "Simulate the IGRA Kaspa Wallet"
)]
struct Args {
    /// Raw transaction in hex bytes prepended by `0x`
    #[arg(short = 't', long)]
    raw_tx: String,

    /// URL of the Ethereum-like client
    #[arg(short = 'u', long)]
    rpc_url: String,
}

#[tokio::main]
async fn main() {
    // Initialize the tracing subscriber
    tracing_subscriber::fmt::init();

    // Parse CLI arguments
    let args = Args::parse();
    info!(
        "Parsed CLI arguments: rpc_url={}, raw_tx=<hidden>",
        args.rpc_url
    );

    // Ensure the raw transaction starts with "0x"
    if !args.raw_tx.starts_with("0x") {
        eprintln!("Error: Raw transaction must start with '0x'");
        info!("Exiting due to invalid raw transaction format.");
        std::process::exit(1);
    }

    info!("Sending raw transaction to the JSON-RPC endpoint...");
    debug!("Raw transaction: {}", args.raw_tx);

    // Send the transaction
    match send_raw_transaction(&args.rpc_url, &args.raw_tx).await {
        Ok(response) => {
            info!("Transaction succeeded: {}", response);
            println!("Success: {response}");
        }
        Err(err) => {
            info!("Transaction failed: {}", err);
            eprintln!("Error: {err}");
        }
    }
}

/// Sends a JSON-RPC request to the EL client with the `eth_sendRawTransaction` method
async fn send_raw_transaction(rpc_url: &str, raw_tx: &str) -> Result<String, String> {
    info!("Preparing JSON-RPC payload...");
    let payload = json!({
        "jsonrpc": "2.0",
        "method": "eth_sendRawTransaction",
        "params": [raw_tx],
        "id": 1
    });
    debug!("JSON-RPC payload: {}", payload);

    // Create an HTTP client
    let client = Client::new();
    info!("Sending HTTP POST request to {}", rpc_url);

    let response = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            let error_message = format!("Failed to send request: {e}");
            info!("{}", error_message);
            error_message
        })?;

    let status = response.status();
    let response_text = response.text().await.map_err(|e| {
        let error_message = format!("Failed to read response: {e}");
        info!("{}", error_message);
        error_message
    })?;

    // Log the raw HTTP response and status
    debug!("HTTP Status: {}", status);
    debug!("Raw response text: {}", response_text);

    // Handle non-success status
    if !status.is_success() {
        let error_message = format!(
            "RPC endpoint returned an error (status {}): {}",
            status.as_u16(),
            response_text
        );
        info!("{}", error_message);
        return Err(error_message);
    }

    info!("Parsing JSON-RPC response...");
    let json_response: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
        let error_message = format!("Failed to parse response JSON: {e}");
        info!("{}", error_message);
        error_message
    })?;

    // Extract the result or error
    if let Some(result) = json_response.get("result") {
        debug!("Parsed result from response: {}", result);
        Ok(result.to_string())
    } else if let Some(error) = json_response.get("error") {
        let error_message = format!("RPC error: {error}");
        info!("{}", error_message);
        Err(error_message)
    } else {
        let error_message = "Unexpected response format".to_string();
        info!("{}", error_message);
        Err(error_message)
    }
}
