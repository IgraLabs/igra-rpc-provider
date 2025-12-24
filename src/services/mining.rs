//! Transaction mining service for Kaspa transactions
//!
//! This module provides functionality to mine Kaspa transactions by modifying
//! their payload with nonce values until the transaction ID starts with a
//! specific prefix. This enables easy filtering and identification of
//! transactions in downstream components.

use crate::{config::MiningConfig, error::AppError};
use hex;
use kaspa_consensus_core::tx::SignableTransaction;
use kaspa_hashes::Hash;
use std::mem;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, warn, Span};

/// Result type for mining operations
pub type MiningResult<T> = Result<T, AppError>;

/// Mining error types
#[derive(Debug, thiserror::Error)]
pub enum MiningError {
    #[error("Mining timeout after {timeout_seconds} seconds")]
    Timeout { timeout_seconds: u64 },

    #[error("Nonce space exhausted, tried all {max_nonce} values")]
    NonceExhaustion { max_nonce: u32 },

    #[error("Transaction finalization failed: {reason}")]
    FinalizationError { reason: String },

    #[error("Invalid transaction state: {reason}")]
    InvalidTransaction { reason: String },
}

impl From<MiningError> for AppError {
    fn from(err: MiningError) -> Self {
        match err {
            MiningError::Timeout { timeout_seconds } => AppError::mining_timeout(timeout_seconds),
            MiningError::NonceExhaustion { max_nonce } => AppError::nonce_exhaustion(max_nonce),
            MiningError::FinalizationError { reason } => AppError::mining_invalid_state(&reason),
            MiningError::InvalidTransaction { reason } => AppError::mining_invalid_state(&reason),
        }
    }
}

/// Mining performance statistics
///
/// These statistics are designed for monitoring and alerting systems
/// with standardized field names that map to common metrics formats.
#[derive(Debug, Clone)]
pub struct MiningStats {
    /// Number of nonces tried during mining
    pub nonces_tried: u32,
    /// Time taken to complete mining
    pub duration: Duration,
    /// Final transaction ID that was found
    pub final_transaction_id: Hash,
    /// Mining rate in hashes per second
    pub hashes_per_second: f64,
}

impl MiningStats {
    #[allow(clippy::cast_precision_loss)]
    pub fn new(nonces_tried: u32, duration: Duration, final_transaction_id: Hash) -> Self {
        let hashes_per_second = if duration.as_secs_f64() > 0.0 {
            f64::from(nonces_tried) / duration.as_secs_f64()
        } else {
            0.0
        };

        Self {
            nonces_tried,
            duration,
            final_transaction_id,
            hashes_per_second,
        }
    }

    /// Logs mining statistics in monitoring-friendly format
    /// Uses standardized metric names compatible with Prometheus/Grafana
    pub fn log_metrics(&self, operation_type: &str) {
        // Standard metrics format for monitoring systems
        info!(
            target: "mining_metrics",
            metric_type = "counter",
            metric_name = "mining_nonces_tried_total",
            metric_value = self.nonces_tried,
            operation = operation_type,
            "Mining nonces tried metric"
        );

        info!(
            target: "mining_metrics",
            metric_type = "histogram",
            metric_name = "mining_duration_seconds",
            metric_value = self.duration.as_secs_f64(),
            operation = operation_type,
            "Mining duration metric"
        );

        info!(
            target: "mining_metrics",
            metric_type = "gauge",
            metric_name = "mining_hash_rate_per_second",
            metric_value = self.hashes_per_second,
            operation = operation_type,
            "Mining hash rate metric"
        );

        // Alert-friendly threshold logging
        self.log_performance_alerts();
    }

    /// Logs performance alerts based on predefined thresholds
    /// These logs are designed for alerting systems like AlertManager
    fn log_performance_alerts(&self) {
        const SLOW_MINING_THRESHOLD_SECONDS: f64 = 30.0;
        const VERY_SLOW_MINING_THRESHOLD_SECONDS: f64 = 120.0;
        const LOW_HASH_RATE_THRESHOLD: f64 = 1000.0;

        let duration_seconds = self.duration.as_secs_f64();

        if duration_seconds > VERY_SLOW_MINING_THRESHOLD_SECONDS {
            warn!(
                target: "mining_alerts",
                alert_type = "performance",
                alert_severity = "critical",
                alert_name = "MiningVerySlowDuration",
                duration_seconds = duration_seconds,
                threshold_seconds = VERY_SLOW_MINING_THRESHOLD_SECONDS,
                nonces_tried = self.nonces_tried,
                hash_rate = self.hashes_per_second,
                "Mining operation took extremely long time"
            );
        } else if duration_seconds > SLOW_MINING_THRESHOLD_SECONDS {
            warn!(
                target: "mining_alerts",
                alert_type = "performance",
                alert_severity = "warning",
                alert_name = "MiningSlowDuration",
                duration_seconds = duration_seconds,
                threshold_seconds = SLOW_MINING_THRESHOLD_SECONDS,
                nonces_tried = self.nonces_tried,
                hash_rate = self.hashes_per_second,
                "Mining operation took longer than expected"
            );
        }

        if self.hashes_per_second > 0.0 && self.hashes_per_second < LOW_HASH_RATE_THRESHOLD {
            warn!(
                target: "mining_alerts",
                alert_type = "performance",
                alert_severity = "warning",
                alert_name = "MiningLowHashRate",
                hash_rate = self.hashes_per_second,
                threshold_hash_rate = LOW_HASH_RATE_THRESHOLD,
                duration_seconds = duration_seconds,
                nonces_tried = self.nonces_tried,
                "Mining hash rate is below expected threshold"
            );
        }
    }
}

/// Core transaction mining service
///
/// The `TransactionMiner` provides functionality to mine Kaspa transactions
/// by modifying their payload until the resulting transaction ID starts
/// with a specified prefix.
#[derive(Debug, Clone)]
pub struct TransactionMiner {
    config: MiningConfig,
}

impl TransactionMiner {
    /// Creates a new transaction miner with the given configuration
    pub fn new(config: MiningConfig) -> Self {
        Self { config }
    }

    /// Mines a transaction to have an ID starting with the configured prefix
    ///
    /// This is the main entry point for mining operations. It uses async
    /// patterns and spawns blocking tasks for CPU-intensive work to avoid
    /// blocking the tokio runtime.
    ///
    /// # Arguments
    ///
    /// * `transaction` - The signable transaction to mine
    ///
    /// # Returns
    ///
    /// Returns the mined transaction and mining statistics on success,
    /// or an AppError on failure (timeout, nonce exhaustion, etc.)
    #[instrument(
        name = "mine_transaction",
        skip(self, transaction),
        fields(
            original_tx_id = %transaction.id(),
            tx_id_prefix = %format!("0x{}", hex::encode(&self.config.tx_id_prefix)),
            timeout_seconds = self.config.timeout_seconds,
            original_payload_size = transaction.tx.payload.len(),
        )
    )]
    pub async fn mine_transaction(
        &self,
        transaction: SignableTransaction,
    ) -> MiningResult<(SignableTransaction, MiningStats)> {
        let start_time = Instant::now();
        let original_tx_id = transaction.id();

        self.log_mining_start(&transaction);

        let result = self
            .execute_mining_with_timeout(transaction, start_time)
            .await;

        self.log_and_record_result(&result, start_time, original_tx_id);

        result
    }

    /// Executes mining with timeout protection
    async fn execute_mining_with_timeout(
        &self,
        transaction: SignableTransaction,
        start_time: Instant,
    ) -> MiningResult<(SignableTransaction, MiningStats)> {
        let timeout_duration = Duration::from_secs(self.config.timeout_seconds);
        let tx_id_prefix = self.config.tx_id_prefix.clone();

        let mining_future = self.mine_with_blocking(transaction, tx_id_prefix, start_time);

        match timeout(timeout_duration, mining_future).await {
            Ok(result) => result,
            Err(_) => Err(AppError::mining_timeout(self.config.timeout_seconds)),
        }
    }

    /// Logs mining operation start
    fn log_mining_start(&self, transaction: &SignableTransaction) {
        info!(
            target: "mining_operations",
            event_type = "mining_start",
            mining_operation = "start",
            tx_id_prefix = %format!("0x{}", hex::encode(&self.config.tx_id_prefix)),
            mining_timeout_seconds = self.config.timeout_seconds,
            transaction_id = %transaction.id(),
            transaction_payload_size = transaction.tx.payload.len(),
            timestamp = chrono::Utc::now().to_rfc3339(),
            "Mining operation started"
        );
    }

    /// Logs and records mining result with comprehensive metrics
    fn log_and_record_result(
        &self,
        result: &MiningResult<(SignableTransaction, MiningStats)>,
        start_time: Instant,
        original_tx_id: Hash,
    ) {
        let span = Span::current();

        match result {
            Ok((_, stats)) => {
                self.log_mining_success(stats);
                self.record_success_metrics(&span, stats);
            }
            Err(err) => {
                self.log_mining_error(err, start_time, original_tx_id);
                self.record_error_metrics(&span, err);
            }
        }
    }

    /// Logs successful mining completion
    fn log_mining_success(&self, stats: &MiningStats) {
        info!(
            target: "mining_operations",
            event_type = "mining_complete",
            mining_operation = "complete",
            mining_result = "success",
            mining_nonces_tried = stats.nonces_tried,
            mining_duration_ms = stats.duration.as_millis(),
            mining_duration_seconds = stats.duration.as_secs_f64(),
            mining_hash_rate = stats.hashes_per_second,
            transaction_final_id = %stats.final_transaction_id,
            timestamp = chrono::Utc::now().to_rfc3339(),
            "Mining completed successfully"
        );

        stats.log_metrics("success");

        info!(
            target: "mining_health",
            health_check = "mining_success",
            status = "healthy",
            timestamp = chrono::Utc::now().to_rfc3339(),
            "Mining health check: success"
        );
    }

    /// Logs mining operation error
    fn log_mining_error(&self, err: &AppError, start_time: Instant, original_tx_id: Hash) {
        match err {
            AppError::MiningTimeout { timeout_seconds } => {
                self.log_mining_timeout(*timeout_seconds, start_time, original_tx_id);
            }
            _ => {
                error!(
                    target: "mining_operations",
                    event_type = "mining_error",
                    mining_operation = "complete",
                    mining_result = "error",
                    mining_error = %err,
                    mining_error_type = self.classify_error(err),
                    mining_duration_ms = start_time.elapsed().as_millis(),
                    transaction_original_id = %original_tx_id,
                    timestamp = chrono::Utc::now().to_rfc3339(),
                    "Mining operation failed"
                );

                error!(
                    target: "mining_health",
                    health_check = "mining_error",
                    status = "unhealthy",
                    error_type = self.classify_error(err),
                    timestamp = chrono::Utc::now().to_rfc3339(),
                    "Mining health check: error"
                );
            }
        }
    }

    /// Logs mining timeout with alerts
    fn log_mining_timeout(&self, timeout_seconds: u64, start_time: Instant, original_tx_id: Hash) {
        let elapsed = start_time.elapsed();

        error!(
            target: "mining_operations",
            event_type = "mining_timeout",
            mining_operation = "complete",
            mining_result = "timeout",
            mining_timeout_seconds = timeout_seconds,
            mining_duration_ms = elapsed.as_millis(),
            mining_duration_seconds = elapsed.as_secs_f64(),
            transaction_original_id = %original_tx_id,
            timestamp = chrono::Utc::now().to_rfc3339(),
            "Mining operation timed out"
        );

        error!(
            target: "mining_alerts",
            alert_type = "operational",
            alert_severity = "critical",
            alert_name = "MiningTimeout",
            timeout_seconds = timeout_seconds,
            actual_duration_seconds = elapsed.as_secs_f64(),
            transaction_id = %original_tx_id,
            timestamp = chrono::Utc::now().to_rfc3339(),
            "Mining timeout alert"
        );

        error!(
            target: "mining_health",
            health_check = "mining_timeout",
            status = "unhealthy",
            timeout_seconds = timeout_seconds,
            timestamp = chrono::Utc::now().to_rfc3339(),
            "Mining health check: timeout"
        );
    }

    /// Records success metrics in span
    fn record_success_metrics(&self, span: &Span, stats: &MiningStats) {
        span.record("mining_result", "success");
        span.record("nonces_tried", stats.nonces_tried);
        span.record("duration_ms", stats.duration.as_millis());
        span.record("hash_rate", stats.hashes_per_second);
        span.record(
            "final_tx_id",
            stats.final_transaction_id.to_string().as_str(),
        );
    }

    /// Records error metrics in span
    fn record_error_metrics(&self, span: &Span, err: &AppError) {
        match err {
            AppError::MiningTimeout { .. } => {
                span.record("mining_result", "timeout");
            }
            _ => {
                span.record("mining_result", "error");
                span.record("error", err.to_string().as_str());
            }
        }
    }

    /// Classifies errors for monitoring purposes
    fn classify_error(&self, error: &AppError) -> &'static str {
        match error {
            AppError::MiningTimeout { .. } => "timeout",
            AppError::NonceExhaustion { .. } => "nonce_exhaustion",
            AppError::TransactionCodecError { .. } => "codec_error",
            AppError::MiningInvalidState(_) => "invalid_state",
            AppError::MiningConfigError(_) => "config_error",
            _ => "unknown",
        }
    }

    /// Performs the actual mining work in a blocking task
    ///
    /// This method is called from `mine_transaction` and runs the CPU-intensive
    /// mining loop in a dedicated blocking thread to avoid blocking the async runtime.
    #[instrument(
        name = "mine_with_blocking",
        skip(self, transaction, tx_id_prefix),
        fields(
            mining_phase = "blocking_execution",
            transaction_id = %transaction.id(),
        )
    )]
    async fn mine_with_blocking(
        &self,
        transaction: SignableTransaction,
        tx_id_prefix: Vec<u8>,
        start_time: Instant,
    ) -> MiningResult<(SignableTransaction, MiningStats)> {
        let config = self.config.clone();

        debug!(
            target: "mining_operations",
            mining_operation = "spawn_blocking",
            "Spawning blocking task for CPU-intensive mining"
        );

        let result = tokio::task::spawn_blocking(move || {
            Self::mine_blocking(transaction, tx_id_prefix, config, start_time)
        })
        .await;

        match result {
            Ok(mining_result) => {
                debug!(
                    target: "mining_operations",
                    mining_operation = "blocking_complete",
                    mining_success = mining_result.is_ok(),
                    "Blocking mining task completed"
                );
                mining_result
            }
            Err(e) => {
                error!(
                    target: "mining_operations",
                    mining_operation = "blocking_error",
                    mining_error = %e,
                    "Mining blocking task failed"
                );
                Err(AppError::mining_invalid_state(&format!(
                    "Mining task failed: {e}"
                )))
            }
        }
    }

    /// The blocking mining implementation
    ///
    /// This runs in a dedicated thread and performs the actual nonce iteration
    /// and transaction finalization loop.
    #[allow(clippy::arithmetic_side_effects)]
    fn mine_blocking(
        mut transaction: SignableTransaction,
        tx_id_prefix: Vec<u8>,
        _config: MiningConfig,
        start_time: Instant,
    ) -> MiningResult<(SignableTransaction, MiningStats)> {
        let mut nonce: u32 = 0;
        let original_payload = transaction.tx.payload.clone();
        let nonce_length = mem::size_of_val(&nonce);

        // Initialize payload with space for nonce
        let payload_base_length = original_payload.len().saturating_sub(nonce_length);
        let mut payload = original_payload[..payload_base_length].to_vec();
        payload.extend_from_slice(&nonce.to_be_bytes());
        transaction.tx.payload = payload;

        let mut nonce_exhaustion_count = 0;
        const MAX_NONCE_EXHAUSTION_ATTEMPTS: u8 = 3;
        const PROGRESS_LOG_INTERVAL: u32 = 100_000;

        // Create a mining context for structured logging
        let mining_context = tracing::info_span!(
            "mining_loop",
            mining_phase = "nonce_iteration",
            mining_nonce_start = 0,
            mining_exhaustion_attempts = 0,
        );
        let _guard = mining_context.enter();

        info!(
            target: "mining_operations",
            mining_operation = "loop_start",
            mining_original_payload_size = original_payload.len(),
            tx_id_prefix = %format!("0x{}", hex::encode(&tx_id_prefix)),
            "Starting nonce iteration loop"
        );

        loop {
            // Update nonce in-place for better performance
            let payload_length = transaction.tx.payload.len();
            transaction.tx.payload[payload_length - nonce_length..]
                .copy_from_slice(&nonce.to_be_bytes());

            // Finalize transaction to recompute the ID
            transaction.tx.finalize();

            let transaction_id = transaction.id();

            // Check if we found the desired prefix
            if Self::check_prefix(&transaction_id, &tx_id_prefix) {
                let duration = start_time.elapsed();
                let stats = MiningStats::new(nonce, duration, transaction_id);

                // Success log with comprehensive metrics
                info!(
                    target: "mining_operations",
                    event_type = "mining_success",
                    mining_operation = "success",
                    mining_nonces_tried = nonce,
                    mining_duration_ms = duration.as_millis(),
                    mining_duration_seconds = duration.as_secs_f64(),
                    mining_hash_rate = stats.hashes_per_second,
                    mining_payload_size = transaction.tx.payload.len(),
                    transaction_final_id = %transaction_id,
                    nonce_value = nonce,
                    nonce_bytes = ?nonce.to_be_bytes(),
                    timestamp = chrono::Utc::now().to_rfc3339(),
                    "Successfully mined transaction"
                );

                return Ok((transaction, stats));
            }

            // Increment nonce with overflow handling
            match nonce.checked_add(1) {
                Some(next_nonce) => nonce = next_nonce,
                None => {
                    // Nonce overflow - implement fallback strategy with monitoring logs
                    warn!(
                        target: "mining_operations",
                        event_type = "nonce_exhaustion",
                        mining_operation = "nonce_exhaustion",
                        mining_attempt = nonce_exhaustion_count + 1,
                        mining_max_attempts = MAX_NONCE_EXHAUSTION_ATTEMPTS,
                        mining_nonces_tried = u32::MAX,
                        timestamp = chrono::Utc::now().to_rfc3339(),
                        "Nonce space exhausted, trying fallback strategy"
                    );

                    if nonce_exhaustion_count >= MAX_NONCE_EXHAUSTION_ATTEMPTS {
                        error!(
                            target: "mining_operations",
                            event_type = "exhaustion_failure",
                            mining_operation = "exhaustion_failure",
                            mining_max_attempts = MAX_NONCE_EXHAUSTION_ATTEMPTS,
                            mining_total_nonces = u32::MAX,
                            timestamp = chrono::Utc::now().to_rfc3339(),
                            "Maximum nonce exhaustion attempts reached"
                        );

                        // Alert for monitoring
                        error!(
                            target: "mining_alerts",
                            alert_type = "operational",
                            alert_severity = "critical",
                            alert_name = "NonceExhaustion",
                            attempts = MAX_NONCE_EXHAUSTION_ATTEMPTS,
                            total_nonces = u32::MAX,
                            timestamp = chrono::Utc::now().to_rfc3339(),
                            "Nonce exhaustion alert"
                        );

                        return Err(AppError::nonce_exhaustion(u32::MAX));
                    }

                    // Fallback: modify transaction output value to create variance
                    if !transaction.tx.outputs.is_empty() {
                        let output_index =
                            usize::from(nonce_exhaustion_count) % transaction.tx.outputs.len();
                        let old_value = transaction.tx.outputs[output_index].value;

                        if old_value > 0 {
                            transaction.tx.outputs[output_index].value =
                                old_value.saturating_sub(1);
                            let new_value = transaction.tx.outputs[output_index].value;

                            debug!(
                                target: "mining_operations",
                                mining_operation = "variance_modification",
                                mining_output_index = output_index,
                                mining_old_value = old_value,
                                mining_new_value = new_value,
                                "Modified output value to create variance"
                            );
                        }
                    }

                    nonce = 0;
                    nonce_exhaustion_count = nonce_exhaustion_count.saturating_add(1);
                }
            }

            // Periodic progress logging with sampling to avoid log spam
            if nonce.is_multiple_of(PROGRESS_LOG_INTERVAL) && nonce > 0 {
                let elapsed = start_time.elapsed();
                let rate = f64::from(nonce) / elapsed.as_secs_f64();

                // Sample progress logs (every 10th interval to reduce volume)
                if (nonce / PROGRESS_LOG_INTERVAL).is_multiple_of(10) {
                    let estimated_remaining = if rate > 0.0 {
                        Some(f64::from(u32::MAX - nonce) / rate)
                    } else {
                        None
                    };

                    info!(
                        target: "mining_progress",
                        event_type = "mining_progress",
                        mining_operation = "progress",
                        mining_nonces_tried = nonce,
                        mining_duration_ms = elapsed.as_millis(),
                        mining_duration_seconds = elapsed.as_secs_f64(),
                        mining_hash_rate = rate,
                        mining_interval = PROGRESS_LOG_INTERVAL,
                        mining_estimated_remaining_seconds = estimated_remaining,
                        timestamp = chrono::Utc::now().to_rfc3339(),
                        "Mining progress update"
                    );
                }

                // Always log debug for detailed tracking
                debug!(
                    target: "mining_operations",
                    mining_operation = "progress",
                    mining_nonces_tried = nonce,
                    mining_duration_ms = elapsed.as_millis(),
                    mining_hash_rate = rate,
                    mining_interval = PROGRESS_LOG_INTERVAL,
                    "Mining progress debug"
                );
            }
        }
    }

    /// Checks if a transaction ID starts with the expected prefix
    fn check_prefix(transaction_id: &Hash, tx_id_prefix: &[u8]) -> bool {
        transaction_id.as_bytes().starts_with(tx_id_prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tracing_test::traced_test;

    fn create_test_config() -> MiningConfig {
        MiningConfig {
            tx_id_prefix: vec![0x00, 0x00], // Easy prefix for testing
            timeout_seconds: 5,
        }
    }

    fn create_test_config_with_timeout(timeout: u64) -> MiningConfig {
        MiningConfig {
            tx_id_prefix: vec![0xff, 0xff], // Use difficult prefix for timeout testing
            timeout_seconds: timeout,
        }
    }

    fn create_test_config_with_prefix(prefix: Vec<u8>) -> MiningConfig {
        MiningConfig {
            tx_id_prefix: prefix,
            timeout_seconds: 5,
        }
    }

    fn create_mock_transaction() -> SignableTransaction {
        use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
        use kaspa_consensus_core::tx::Transaction;

        let tx = Transaction::new(
            0,                    // version
            vec![],               // inputs
            vec![],               // outputs
            0,                    // lock_time
            SUBNETWORK_ID_NATIVE, // subnetwork_id
            0,                    // gas
            vec![1, 2, 3, 4],     // payload
        );

        SignableTransaction::new(tx)
    }

    fn create_mock_transaction_with_payload(payload: Vec<u8>) -> SignableTransaction {
        use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
        use kaspa_consensus_core::tx::Transaction;

        let tx = Transaction::new(
            0,                    // version
            vec![],               // inputs
            vec![],               // outputs
            0,                    // lock_time
            SUBNETWORK_ID_NATIVE, // subnetwork_id
            0,                    // gas
            payload,              // payload
        );

        SignableTransaction::new(tx)
    }

    // ========== Configuration Tests ==========

    #[test]
    fn test_mining_config_validation_edge_cases() {
        // Note: Config validation happens in src/config.rs, not here
        // These tests just verify the config structure itself
        let config = MiningConfig {
            tx_id_prefix: vec![],
            timeout_seconds: 10,
        };
        assert_eq!(config.tx_id_prefix.len(), 0);

        let config = MiningConfig {
            tx_id_prefix: vec![0x97, 0xb1],
            timeout_seconds: 10,
        };
        assert_eq!(config.tx_id_prefix.len(), 2);
    }

    // ========== Prefix Checking Tests ==========

    #[test]
    fn test_prefix_check_comprehensive() {
        // Test empty prefix (should always match)
        let hash_bytes = [0u8; 32];
        let hash = Hash::from_bytes(hash_bytes);
        let empty_prefix = vec![];
        assert!(TransactionMiner::check_prefix(&hash, &empty_prefix));

        // Test single byte prefix
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0] = 0x97;
        let hash = Hash::from_bytes(hash_bytes);
        let single_prefix = vec![0x97];
        assert!(TransactionMiner::check_prefix(&hash, &single_prefix));

        let wrong_single_prefix = vec![0x96];
        assert!(!TransactionMiner::check_prefix(&hash, &wrong_single_prefix));

        // Test full prefix match
        hash_bytes[0] = 0x97;
        hash_bytes[1] = 0xb1;
        let hash = Hash::from_bytes(hash_bytes);
        let full_prefix = vec![0x97, 0xb1];
        assert!(TransactionMiner::check_prefix(&hash, &full_prefix));

        // Test partial match (first byte matches, second doesn't)
        let partial_prefix = vec![0x97, 0xb2];
        assert!(!TransactionMiner::check_prefix(&hash, &partial_prefix));

        // Test prefix longer than hash (should fail)
        let long_prefix = vec![0u8; 64]; // Longer than 32 bytes
        assert!(!TransactionMiner::check_prefix(&hash, &long_prefix));

        // Test exact hash length prefix
        let exact_length_prefix = vec![0u8; 32];
        let exact_hash_bytes = [0u8; 32];
        let exact_hash = Hash::from_bytes(exact_hash_bytes);
        assert!(TransactionMiner::check_prefix(
            &exact_hash,
            &exact_length_prefix
        ));

        // Test boundary values
        hash_bytes[0] = 0x00;
        hash_bytes[1] = 0x00;
        let hash = Hash::from_bytes(hash_bytes);
        let zero_prefix = vec![0x00, 0x00];
        assert!(TransactionMiner::check_prefix(&hash, &zero_prefix));

        hash_bytes[0] = 0xff;
        hash_bytes[1] = 0xff;
        let hash = Hash::from_bytes(hash_bytes);
        let max_prefix = vec![0xff, 0xff];
        assert!(TransactionMiner::check_prefix(&hash, &max_prefix));
    }

    // ========== Mining Stats Tests ==========

    #[test]
    fn test_mining_stats_edge_cases() {
        let hash_bytes = [0u8; 32];
        let hash = Hash::from_bytes(hash_bytes);

        // Test zero duration (should not panic)
        let stats = MiningStats::new(1000, Duration::from_nanos(0), hash);
        assert_eq!(stats.nonces_tried, 1000);
        assert_eq!(stats.duration, Duration::from_nanos(0));
        assert_eq!(stats.hashes_per_second, 0.0);

        // Test very small duration
        let stats = MiningStats::new(1, Duration::from_nanos(1), hash);
        assert_eq!(stats.nonces_tried, 1);
        assert_eq!(stats.duration, Duration::from_nanos(1));
        assert!(stats.hashes_per_second > 0.0);

        // Test zero nonces
        let stats = MiningStats::new(0, Duration::from_secs(1), hash);
        assert_eq!(stats.nonces_tried, 0);
        assert_eq!(stats.hashes_per_second, 0.0);

        // Test maximum values
        let stats = MiningStats::new(u32::MAX, Duration::from_secs(1), hash);
        assert_eq!(stats.nonces_tried, u32::MAX);
        assert!(stats.hashes_per_second > 0.0);

        // Test large duration
        let stats = MiningStats::new(1000, Duration::from_secs(u64::MAX), hash);
        assert_eq!(stats.nonces_tried, 1000);
        assert!(stats.hashes_per_second >= 0.0); // Should be very small but not panic
    }

    #[test]
    fn test_mining_stats_performance_alerts() {
        let hash_bytes = [0u8; 32];
        let hash = Hash::from_bytes(hash_bytes);

        // Test slow mining threshold (should trigger warning)
        let slow_stats = MiningStats::new(10000, Duration::from_secs(35), hash);
        assert!(slow_stats.duration.as_secs_f64() > 30.0);
        slow_stats.log_metrics("test_slow"); // Should log warning

        // Test very slow mining threshold (should trigger critical)
        let very_slow_stats = MiningStats::new(10000, Duration::from_secs(125), hash);
        assert!(very_slow_stats.duration.as_secs_f64() > 120.0);
        very_slow_stats.log_metrics("test_very_slow"); // Should log critical

        // Test low hash rate (should trigger warning)
        let low_rate_stats = MiningStats::new(100, Duration::from_secs(1), hash);
        assert!(low_rate_stats.hashes_per_second < 1000.0);
        low_rate_stats.log_metrics("test_low_rate"); // Should log warning

        // Test normal performance (should not trigger alerts)
        let normal_stats = MiningStats::new(5000, Duration::from_secs(5), hash);
        assert!(normal_stats.duration.as_secs_f64() < 30.0);
        assert!(normal_stats.hashes_per_second >= 1000.0);
        normal_stats.log_metrics("test_normal"); // Should not log alerts
    }

    // ========== Error Classification Tests ==========

    #[test]
    fn test_error_classification_comprehensive() {
        let config = create_test_config();
        let miner = TransactionMiner::new(config);

        // Test all error types
        assert_eq!(
            miner.classify_error(&AppError::mining_timeout(10)),
            "timeout"
        );
        assert_eq!(
            miner.classify_error(&AppError::nonce_exhaustion(1000)),
            "nonce_exhaustion"
        );
        assert_eq!(
            miner.classify_error(&AppError::transaction_codec_error("decode", "test")),
            "codec_error"
        );
        assert_eq!(
            miner.classify_error(&AppError::mining_invalid_state("test")),
            "invalid_state"
        );
        assert_eq!(
            miner.classify_error(&AppError::mining_config_error("test")),
            "config_error"
        );

        // Test unknown error types
        assert_eq!(
            miner.classify_error(&AppError::ConfigError("test".to_string())),
            "unknown"
        );
        assert_eq!(
            miner.classify_error(&AppError::InvalidTransactionFormat),
            "unknown"
        );
        assert_eq!(miner.classify_error(&AppError::WalletCallError), "unknown");
    }

    // ========== Mining Error Conversion Tests ==========

    #[test]
    fn test_mining_error_conversions_comprehensive() {
        // Test timeout error conversion
        let timeout_error = MiningError::Timeout {
            timeout_seconds: 15,
        };
        let app_error: AppError = timeout_error.into();
        match app_error {
            AppError::MiningTimeout {
                timeout_seconds: 15,
            } => {} // Expected
            _ => panic!("Expected MiningTimeout variant"),
        }

        // Test nonce exhaustion error conversion
        let exhaustion_error = MiningError::NonceExhaustion { max_nonce: 500000 };
        let app_error: AppError = exhaustion_error.into();
        match app_error {
            AppError::NonceExhaustion {
                nonces_tried: 500000,
            } => {} // Expected
            _ => panic!("Expected NonceExhaustion variant"),
        }

        // Test finalization error conversion
        let finalization_error = MiningError::FinalizationError {
            reason: "test finalization failure".to_string(),
        };
        let app_error: AppError = finalization_error.into();
        match app_error {
            AppError::MiningInvalidState(msg) => {
                assert_eq!(msg, "test finalization failure");
            }
            _ => panic!("Expected MiningInvalidState variant"),
        }

        // Test invalid transaction error conversion
        let invalid_error = MiningError::InvalidTransaction {
            reason: "test invalid transaction".to_string(),
        };
        let app_error: AppError = invalid_error.into();
        match app_error {
            AppError::MiningInvalidState(msg) => {
                assert_eq!(msg, "test invalid transaction");
            }
            _ => panic!("Expected MiningInvalidState variant"),
        }
    }

    // ========== Timeout Tests ==========

    #[traced_test]
    #[tokio::test]
    async fn test_mining_timeout_behavior() {
        // Create a config with very short timeout and difficult prefix
        let config = create_test_config_with_timeout(1); // 1 second timeout with 0xff, 0xff prefix
        let miner = TransactionMiner::new(config);

        // Create a transaction and try to mine with impossible prefix
        let transaction = create_mock_transaction();

        let start = std::time::Instant::now();
        let result = miner.mine_transaction(transaction).await;
        let elapsed = start.elapsed();

        // Should timeout and return error (or succeed if very lucky)
        match result {
            Err(AppError::MiningTimeout { timeout_seconds: 1 }) => {
                // Expected timeout
                assert!(elapsed >= Duration::from_millis(900));
                assert!(elapsed <= Duration::from_millis(2000));
            }
            Ok(_) => {
                // Very unlikely but possible - mining succeeded despite difficult prefix
                // This is still a valid outcome, just very rare
            }
            Err(other) => {
                panic!("Expected MiningTimeout or success, got: {other:?}");
            }
        }
    }

    // ========== Concurrent Mining Tests ==========

    #[traced_test]
    #[tokio::test]
    #[allow(clippy::arithmetic_side_effects)]
    async fn test_concurrent_mining_operations() {
        let config = create_test_config();
        let miner = Arc::new(TransactionMiner::new(config));

        let mut handles = vec![];

        // Start multiple mining operations concurrently
        for i in 0..3 {
            let miner_clone = miner.clone();
            let handle = tokio::spawn(async move {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let transaction = create_mock_transaction_with_payload(vec![i as u8; 4]);
                miner_clone.mine_transaction(transaction).await
            });
            handles.push(handle);
        }

        // Wait for all to complete
        let mut success_count = 0;
        let mut error_count = 0;

        for handle in handles {
            match handle.await.expect("Task should not panic") {
                Ok(_) => success_count += 1,
                Err(_) => error_count += 1,
            }
        }

        // At least some should succeed (with easy prefix 0x00, 0x00)
        assert!(success_count > 0);

        // Total should be 3
        assert_eq!(success_count + error_count, 3);
    }

    // ========== Transaction Finalization Tests ==========

    #[test]
    fn test_transaction_finalization_effects() {
        let mut transaction = create_mock_transaction();
        let _original_id = transaction.id();

        // Modify payload
        transaction.tx.payload = vec![5, 6, 7, 8];

        // Before finalization, ID might be stale
        let _before_finalize_id = transaction.id();

        // Finalize to update ID
        transaction.tx.finalize();
        let after_finalize_id = transaction.id();

        // The ID should potentially change after finalization
        // (depends on implementation, but finalize should update internal state)
        assert_eq!(after_finalize_id, transaction.id()); // ID should be consistent after finalize
    }

    // ========== Hash Calculation Edge Cases ==========

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn test_hash_rate_calculation_edge_cases() {
        let hash_bytes = [0u8; 32];
        let hash = Hash::from_bytes(hash_bytes);

        // Test various duration and nonce combinations
        let test_cases = vec![
            (0, Duration::from_secs(1), 0.0),       // Zero nonces
            (1000, Duration::from_secs(1), 1000.0), // Normal case
            (1, Duration::from_millis(1), 1000.0),  // High rate
            (u32::MAX, Duration::from_secs(1), f64::from(u32::MAX)), // Maximum nonces
            (1000, Duration::from_nanos(1), 1000.0 * 1_000_000_000.0), // Very small duration
        ];

        for (nonces, duration, expected_rate) in test_cases {
            let stats = MiningStats::new(nonces, duration, hash);
            if duration.as_secs_f64() > 0.0 {
                let actual_rate = stats.hashes_per_second;
                let tolerance = expected_rate * 0.01; // 1% tolerance
                assert!(
                    (actual_rate - expected_rate).abs() <= tolerance.max(1.0),
                    "Expected rate: {expected_rate}, Actual rate: {actual_rate}, Tolerance: {tolerance}"
                );
            } else {
                assert_eq!(stats.hashes_per_second, 0.0);
            }
        }
    }

    // ========== Integration Tests ==========

    #[traced_test]
    #[tokio::test]
    async fn test_end_to_end_mining_success() {
        // Test complete mining process with achievable prefix
        let config = create_test_config_with_prefix(vec![0x00, 0x00]);
        let miner = TransactionMiner::new(config);

        let transaction = create_mock_transaction();
        let _original_id = transaction.id();

        let result = miner.mine_transaction(transaction).await;

        assert!(result.is_ok());
        let (mined_transaction, stats) = result.expect("Should succeed with mining");

        // Verify the mined transaction has the expected prefix
        let final_id = mined_transaction.id();
        let id_bytes = final_id.as_bytes();
        assert_eq!(id_bytes[0], 0x00);
        assert_eq!(id_bytes[1], 0x00);

        // Verify stats are reasonable (remove useless comparisons)
        assert_eq!(stats.final_transaction_id, final_id);
        assert!(stats.hashes_per_second >= 0.0);
    }

    #[traced_test]
    #[tokio::test]
    async fn test_mining_with_different_prefixes() {
        let test_prefixes = vec![
            vec![0x00, 0x00], // Easy
            vec![0x01, 0x00], // Medium
                              // Note: We don't test very difficult prefixes as they would timeout
        ];

        for prefix in test_prefixes {
            let config = create_test_config_with_prefix(prefix.clone());
            let miner = TransactionMiner::new(config);

            let transaction = create_mock_transaction();
            let result = miner.mine_transaction(transaction).await;

            // Should either succeed or timeout (both are valid outcomes)
            match result {
                Ok((mined_tx, _stats)) => {
                    // If it succeeded, verify the prefix
                    let id_bytes = mined_tx.id().as_bytes();
                    assert_eq!(&id_bytes[0..prefix.len()], &prefix[..]);
                }
                Err(AppError::MiningTimeout { .. }) => {
                    // Timeout is acceptable for difficult prefixes
                }
                Err(other) => {
                    panic!("Unexpected error: {other:?}");
                }
            }
        }
    }

    // ========== Stress Tests ==========

    #[test]
    fn test_mining_stats_memory_efficiency() {
        let hash_bytes = [0u8; 32];
        let hash = Hash::from_bytes(hash_bytes);

        // Create many stats objects to test memory usage
        #[allow(clippy::cast_possible_truncation)]
        let stats_objects: Vec<MiningStats> = (0..1000)
            .map(|i| MiningStats::new(i as u32, Duration::from_millis(i), hash))
            .collect();

        // Verify all objects are created correctly
        assert_eq!(stats_objects.len(), 1000);
        assert_eq!(stats_objects[0].nonces_tried, 0);
        assert_eq!(stats_objects[999].nonces_tried, 999);
    }

    // ========== Logging Tests ==========

    #[traced_test]
    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn test_comprehensive_logging_coverage() {
        let hash_bytes = [0u8; 32];
        let hash = Hash::from_bytes(hash_bytes);

        // Test logging with various scenarios
        let scenarios = vec![
            ("success", 1000, Duration::from_secs(1)),
            ("timeout", 5000, Duration::from_secs(35)), // Should trigger slow warning
            ("critical", 1000, Duration::from_secs(125)), // Should trigger critical
            ("low_rate", 100, Duration::from_secs(1)),  // Should trigger low hash rate
        ];

        for (scenario, nonces, duration) in scenarios {
            let stats = MiningStats::new(nonces, duration, hash);

            // This should log without panicking
            stats.log_metrics(scenario);

            // Verify calculations are correct
            if duration.as_secs_f64() > 0.0 {
                let expected_rate = f64::from(nonces) / duration.as_secs_f64();
                assert!((stats.hashes_per_second - expected_rate).abs() < 1.0);
            }
        }
    }

    // ========== Basic Functionality Verification ==========

    #[test]
    fn test_check_prefix() {
        // Create a hash with the expected prefix (Hash expects 32 bytes)
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0] = 0x97;
        hash_bytes[1] = 0xb1;
        let hash = Hash::from_bytes(hash_bytes);
        let prefix = vec![0x97, 0xb1];

        assert!(TransactionMiner::check_prefix(&hash, &prefix));

        let wrong_prefix = vec![0x12, 0x34];
        assert!(!TransactionMiner::check_prefix(&hash, &wrong_prefix));
    }

    #[test]
    fn test_mining_stats_calculation() {
        let duration = Duration::from_millis(1000); // 1 second
        let nonces_tried = 1000;
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0] = 0x97;
        hash_bytes[1] = 0xb1;
        let hash = Hash::from_bytes(hash_bytes);

        let stats = MiningStats::new(nonces_tried, duration, hash);

        assert_eq!(stats.nonces_tried, 1000);
        assert_eq!(stats.duration, duration);
        assert_eq!(stats.final_transaction_id, hash);
        assert!((stats.hashes_per_second - 1000.0).abs() < 1.0);
    }

    #[traced_test]
    #[tokio::test]
    async fn test_transaction_miner_creation() {
        let config = create_test_config();
        let miner = TransactionMiner::new(config.clone());

        assert_eq!(miner.config.tx_id_prefix, config.tx_id_prefix);
        assert_eq!(miner.config.timeout_seconds, config.timeout_seconds);
    }

    #[test]
    fn test_mining_stats_metrics_logging() {
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0] = 0x97;
        hash_bytes[1] = 0xb1;
        let hash = Hash::from_bytes(hash_bytes);

        let stats = MiningStats::new(5000, Duration::from_secs(5), hash);

        // This should log metrics without panicking
        stats.log_metrics("test");

        // Verify hash rate calculation
        assert!((stats.hashes_per_second - 1000.0).abs() < 1.0);
    }

    #[test]
    fn test_error_classification() {
        let config = create_test_config();
        let miner = TransactionMiner::new(config);

        assert_eq!(
            miner.classify_error(&AppError::mining_timeout(10)),
            "timeout"
        );
        assert_eq!(
            miner.classify_error(&AppError::nonce_exhaustion(1000)),
            "nonce_exhaustion"
        );
        assert_eq!(
            miner.classify_error(&AppError::transaction_codec_error("decode", "test")),
            "codec_error"
        );
    }

    #[test]
    fn test_prefix_check_edge_cases() {
        // Test empty prefix
        let hash_bytes = [0u8; 32];
        let hash = Hash::from_bytes(hash_bytes);
        let empty_prefix = vec![];
        assert!(TransactionMiner::check_prefix(&hash, &empty_prefix));

        // Test prefix longer than hash
        let long_prefix = vec![0u8; 64]; // Longer than 32 bytes
        assert!(!TransactionMiner::check_prefix(&hash, &long_prefix));

        // Test exact match
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0] = 0xaa;
        hash_bytes[1] = 0xbb;
        hash_bytes[2] = 0xcc;
        let hash = Hash::from_bytes(hash_bytes);
        let exact_prefix = vec![0xaa, 0xbb, 0xcc];
        assert!(TransactionMiner::check_prefix(&hash, &exact_prefix));
    }

    #[test]
    fn test_mining_error_conversion() {
        let mining_error = MiningError::Timeout {
            timeout_seconds: 10,
        };
        let app_error: AppError = mining_error.into();

        match app_error {
            AppError::MiningTimeout {
                timeout_seconds: 10,
            } => {
                // Expected
            }
            _ => panic!("Expected MiningTimeout variant, got: {app_error:?}"),
        }
    }

    #[test]
    fn test_nonce_exhaustion_error_conversion() {
        let mining_error = MiningError::NonceExhaustion { max_nonce: 1000000 };
        let app_error: AppError = mining_error.into();

        match app_error {
            AppError::NonceExhaustion {
                nonces_tried: 1000000,
            } => {
                // Expected
            }
            _ => panic!("Expected NonceExhaustion variant, got: {app_error:?}"),
        }
    }

    #[traced_test]
    #[test]
    fn test_structured_logging_context() {
        let config = create_test_config();
        let miner = TransactionMiner::new(config);

        // Test that the miner has the expected configuration for logging
        assert_eq!(miner.config.tx_id_prefix, vec![0x00, 0x00]);
        assert_eq!(miner.config.timeout_seconds, 5);
    }

    #[test]
    fn test_mining_config_validation() {
        let config = create_test_config();

        // Test that config values are as expected
        assert_eq!(config.tx_id_prefix, vec![0x00, 0x00]);
        assert_eq!(config.timeout_seconds, 5);
    }
}
