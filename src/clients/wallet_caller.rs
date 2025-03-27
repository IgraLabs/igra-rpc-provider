use crate::config::WalletConfig;
use kaswallet_proto::kaswallet_proto::wallet_client::WalletClient;
use kaswallet_proto::kaswallet_proto::{NewAddressRequest, SendRequest, TransactionDescription};
use std::error::Error;
use tokio::sync::Mutex;
use tonic::transport::Channel;
use tracing::info;

pub struct WalletCaller {
    wallet_config: WalletConfig,
    wallet_daemon_client: Mutex<WalletClient<Channel>>,
    to_address: String,
}

impl WalletCaller {
    pub async fn new(wallet_config: WalletConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let mut wallet_daemon_client =
            WalletClient::connect(wallet_config.wallet_daemon_uri.clone()).await?;
        let to_address_response = wallet_daemon_client
            .new_address(NewAddressRequest {})
            .await?;
        Ok(Self {
            wallet_config,
            wallet_daemon_client: Mutex::new(wallet_daemon_client),
            to_address: to_address_response.into_inner().address,
        })
    }

    /// Calls the KASPA Wallet to send to KASPA network a transaction with the L2 payload.
    ///
    /// # Parameters:
    /// - `raw_tx`: A string slice representing the hex-encoded raw transaction to be processed.
    ///
    /// # Returns:
    /// - `Ok(())` if the command executes successfully.
    /// - `Err(String)` if the command fails.
    ///
    /// # Errors:
    /// Returns an error with a string description if there was a problem sending the transaction
    pub async fn send_transaction(
        &self,
        transaction_bytes: &Vec<u8>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let transaction_description = Some(TransactionDescription {
            to_address: self.to_address.clone(),
            amount: 0,
            is_send_all: true,
            payload: transaction_bytes.clone(),
            from_addresses: vec![],
            utxos: vec![],
            use_existing_change_address: false,
            fee_policy: None,
        });

        let mut wallet_daemon_client = self.wallet_daemon_client.lock().await;
        let response = wallet_daemon_client
            .send(SendRequest {
                transaction_description,
                password: self.wallet_config.password.clone(),
            })
            .await?;

        info!(
            "Transaction(s) sent successfully: {:?}",
            response.into_inner().transaction_ids
        );

        Ok(())
    }
}
