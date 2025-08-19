//! Entry Transaction Sender CLI
//!
//! A command-line utility for sending Entry Transactions to the IGRA protocol.
//! Entry Transactions bridge value from L1 (KASPA blockchain) to L2 (IGRA Execution Layer)
//! by locking KAS coins on L1 and issuing an equivalent amount of iKAS on L2.

use clap::Parser;
use igra_rpc_provider::services::entry_transaction::{
    validation, EntryTransactionError, EntryTransactionService,
};
use kaspa_consensus_core::constants::SOMPI_PER_KASPA;
use std::process;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Exit codes for different error types to enable scripting and automation
#[repr(i32)]
#[derive(Debug, Clone, Copy)]
pub enum ExitCode {
    /// Successful execution
    Success = 0,
    /// Input validation errors (invalid arguments, addresses, amounts)
    ValidationError = 1,
    /// Configuration errors (config loading, wallet connection setup)
    ConfigError = 2,
    /// Wallet/network errors (transaction creation, mining, broadcasting)
    WalletError = 3,
}

impl ExitCode {
    /// Exit the process with the appropriate exit code
    pub fn exit(self) -> ! {
        process::exit(self as i32)
    }
}

/// CLI arguments for the Entry Transaction Sender
#[derive(Parser, Debug)]
#[command(
    author = "Igra Labs",
    version = "1.0",
    about = "Send Entry Transactions to bridge KAS from L1 to L2",
    long_about = "
Entry Transaction Sender creates and broadcasts Entry Transactions to the IGRA protocol.
These transactions lock KAS coins on L1 (KASPA blockchain) and issue equivalent iKAS on L2.

Example usage:
  entry-transaction-sender \\
    --recipient kaspa:qpamkvhgh0d8j6yqzx0jk3xnrn0lhkxmcqwgjzn2xrvjjvxkzpq5rj3lqyxn9p2t \\
    --amount 1.5 \\
    --l2-address 0x742d35Cc6634C0532925a3b8D0b16e5E3dd7b9c0

The amount is specified in KAS (e.g., 1.5 for 1.5 KAS).
"
)]
struct Args {
    /// Kaspa address where the locked KAS will be sent
    #[arg(
        short = 'r',
        long = "recipient",
        help = "Kaspa address for the L1 recipient",
        long_help = "The Kaspa address where the locked KAS coins will be sent on L1.
This should be a valid Kaspa address in the format: kaspa:qpam..."
    )]
    recipient: String,

    /// Amount of KAS to transfer (supports decimal values like 1.5)
    #[arg(
        short = 'a',
        long = "amount",
        help = "Amount in KAS (supports decimals like 1.5)",
        long_help = "The amount of KAS to transfer. Supports decimal values.
Examples: 1 (1 KAS), 1.5 (1.5 KAS), 0.00000001 (1 SOMPI)"
    )]
    amount: String,

    /// Ethereum address on L2 where iKAS will be minted (20 bytes)
    #[arg(
        short = 'l',
        long = "l2-address",
        help = "Ethereum address on L2 for iKAS minting",
        long_help = "The Ethereum address on L2 where equivalent iKAS tokens will be minted.
This should be a valid 20-byte Ethereum address (40 hex characters).
The 0x prefix is optional. Example: 0x742d35Cc6634C0532925a3b8D0b16e5E3dd7b9c0"
    )]
    l2_address: String,
}

/// Application runner that coordinates the entire CLI flow
struct App;

impl App {
    async fn run() -> Result<(), EntryTransactionError> {
        let args = Args::parse();

        // Setup logging after args are parsed (so --help/--version work without logging)
        setup_logging();
        info!("IGRA Entry Transaction Sender starting...");
        info!(
            "Arguments parsed: recipient={}, amount={}, l2_address={}",
            args.recipient, args.amount, args.l2_address
        );

        // Validate and parse arguments
        let request = validation::validate_and_parse_request(
            &args.recipient,
            &args.amount,
            &args.l2_address,
        )?;
        info!("Arguments validated successfully");

        // Initialize service and process transaction
        let service = EntryTransactionService::new().await?;
        let start_time = std::time::Instant::now();

        let tx_id = service.process_transaction(&request).await?;
        let duration = start_time.elapsed();

        Self::print_success_message(&request, &tx_id, duration);
        Ok(())
    }

    #[allow(clippy::cast_precision_loss)]
    fn print_success_message(
        request: &igra_rpc_provider::services::entry_transaction::EntryTransactionRequest,
        tx_id: &str,
        duration: std::time::Duration,
    ) {
        // Convert SOMPI to KAS for display
        let kas_amount = request.amount_sompi as f64 / SOMPI_PER_KASPA as f64;

        println!("✅ Entry transaction sent successfully!");
        println!("   Transaction ID: {tx_id}");
        println!("   Recipient: {}", request.recipient);
        println!(
            "   Amount: {:.8} KAS ({} SOMPI)",
            kas_amount, request.amount_sompi
        );
        println!("   L2 Address: 0x{}", hex::encode(request.l2_address));
        println!("   Processing time: {duration:?}");
    }

    fn handle_error(error: EntryTransactionError) -> ! {
        let exit_code = match &error {
            EntryTransactionError::Validation(_) => {
                eprintln!("❌ {error}");
                eprintln!("Use --help for usage information.");
                ExitCode::ValidationError
            }
            EntryTransactionError::Config(_) => {
                eprintln!("❌ {error}");
                eprintln!("Please check your config.toml file.");
                ExitCode::ConfigError
            }
            EntryTransactionError::Wallet(_) | EntryTransactionError::Serialization(_) => {
                eprintln!("❌ {error}");
                ExitCode::WalletError
            }
        };

        error!("Application error: {}", error);
        exit_code.exit();
    }
}

/// Entry point for the Entry Transaction Sender CLI
#[tokio::main]
async fn main() {
    match App::run().await {
        Ok(()) => {
            info!("Entry Transaction Sender completed successfully");
        }
        Err(error) => {
            App::handle_error(error);
        }
    }
}

/// Sets up comprehensive logging
fn setup_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info").add_directive(
            "entry_transaction_sender=debug"
                .parse()
                .expect("Failed to parse logging directive"),
        )
    });

    tracing_subscriber::registry()
        .with(fmt::layer().with_ansi(true).with_target(true))
        .with(env_filter)
        .init();
}
