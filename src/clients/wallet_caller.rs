use crate::config::WalletConfig;
use kaswallet_proto::kaswallet_proto::wallet_client::WalletClient;
use kaswallet_proto::kaswallet_proto::{NewAddressRequest, SendRequest, TransactionDescription};
use std::env;
use std::error::Error;
use tokio::sync::Mutex;
use tonic::transport::Channel;
use tracing::info;
use hex;

const PASSWORD_ENV_VAR: &str = "KASWALLET_PASSWORD";

pub struct WalletCaller {
    wallet_daemon_client: Mutex<WalletClient<Channel>>,
    to_address: String,
    password: String,
}

impl WalletCaller {
    pub async fn new(wallet_config: WalletConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let mut wallet_daemon_client =
            WalletClient::connect(wallet_config.wallet_daemon_uri.clone()).await?;
        let to_address = wallet_config.to_address.clone();
        let to_address = if to_address == "" {
            wallet_daemon_client
                .new_address(NewAddressRequest {})
                .await?
                .into_inner()
                .address
        } else {
            to_address
        };

        // Check for the environment variable with better error handling
        let password = match env::var(PASSWORD_ENV_VAR) {
            Ok(pwd) => pwd,
            Err(env::VarError::NotPresent) => {
                return Err(format!(
                    "Environment variable {} is not set. This is required for wallet authentication.",
                    PASSWORD_ENV_VAR
                ).into());
            },
            Err(env::VarError::NotUnicode(..)) => {
                return Err(format!(
                    "Environment variable {} contains invalid Unicode characters.",
                    PASSWORD_ENV_VAR
                ).into());
            }
        };

        Ok(Self {
            wallet_daemon_client: Mutex::new(wallet_daemon_client),
            to_address,
            password,
        })
    }

    /// Calls the KASPA Wallet to send to KASPA network a transaction with the L2 payload.
    ///
    /// # Parameters:
    /// - `payload`: The transaction payload to send to the KASPA wallet
    /// - `transaction_id`: Optional transaction ID for logging
    /// - `transaction_hash`: Optional transaction hash for logging
    ///
    /// # Returns:
    /// - `Ok(())` if the command executes successfully.
    /// - `Err(String)` if the command fails.
    ///
    /// # Errors:
    /// Returns an error with a string description if there was a problem sending the transaction
    pub async fn send_transaction(
        &self,
        payload: Vec<u8>,
        transaction_id: Option<String>,
        transaction_hash: Option<String>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Log the payload details with full payload
        let payload_size = payload.len();
        let full_payload = format!("0x{}", hex::encode(&payload));

        let transaction_description = Some(TransactionDescription {
            to_address: self.to_address.clone(),
            amount: 0,
            is_send_all: true,
            payload,
            from_addresses: vec![],
            utxos: vec![],
            use_existing_change_address: false,
            fee_policy: None,
        });

        let mut wallet_daemon_client = self.wallet_daemon_client.lock().await;

        // Create log prefix
        let log_prefix = match (transaction_id.as_ref(), transaction_hash.as_ref()) {
            (Some(id), Some(hash)) => format!("[id={}, hash={}]", id, hash),
            (Some(id), None) => format!("[id={}]", id),
            (None, Some(hash)) => format!("[hash={}]", hash),
            (None, None) => String::new(),
        };

        info!(
            "Transaction{} sending to wallet, payload_size={}, payload={}",
            log_prefix, payload_size, full_payload
        );

        let start = std::time::Instant::now();
        let response = wallet_daemon_client
            .send(SendRequest {
                transaction_description,
                password: self.password.clone(),
            })
            .await?;

        let tx_ids = response.into_inner().transaction_ids;
        let duration = start.elapsed();

        info!(
            "Transaction{} sent successfully: {:?}, time={:?}, payload_size={}",
            log_prefix, tx_ids, duration, payload_size
        );

        Ok(())
    }
}
