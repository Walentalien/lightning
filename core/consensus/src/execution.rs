use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use fastcrypto::hash::HashFunction;
use fleek_blake3 as blake3;
use lightning_interfaces::prelude::*;
use lightning_interfaces::types::{
    Block,
    Digest as BroadcastDigest,
    Epoch,
    Event,
    Metadata,
    NodeIndex,
    TransactionRequest,
};
use lightning_interfaces::ExecutionEngineSocket;
use lightning_utils::application::QueryRunnerExt;
use narwhal_crypto::DefaultHashFunction;
use narwhal_executor::ExecutionState;
use narwhal_types::{Batch, BatchDigest, ConsensusOutput, Transaction};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Notify};
use tracing::{error, info};

use crate::consensus::PubSubMsg;
use crate::transaction_store::TransactionStore;

pub type Digest = [u8; 32];

// Exponentially moving average parameter for estimating the time between executions of parcels.
// This parameter must be in range [0, 1].
const TBE_EMA: f64 = 0.125;
// Bounds for the estimated time between executions.
const MIN_TBE: Duration = Duration::from_secs(30);
const MAX_TBE: Duration = Duration::from_secs(40);

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuthenticStampedParcel {
    pub transactions: Vec<Transaction>,
    pub last_executed: Digest,
    pub epoch: Epoch,
    pub sub_dag_index: u64,
}

impl ToDigest for AuthenticStampedParcel {
    fn transcript(&self) -> TranscriptBuilder {
        panic!("We don't need this here");
    }

    fn to_digest(&self) -> Digest {
        let batch_digest =
            BatchDigest::new(DefaultHashFunction::digest_iterator(self.transactions.iter()).into());

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.transactions.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&batch_digest.0);
        bytes.extend_from_slice(&self.last_executed);

        blake3::hash(&bytes).into()
    }
}

/// A message an authority sends out attest that an Authentic stamp parcel is accurate. When an edge
/// node gets 2f+1 of these it commits the transactions in the parcel
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CommitteeAttestation {
    /// The digest we are attesting is correct
    pub digest: Digest,
    /// We send random bytes with this message so it gives it a unique hash and differentiates it
    /// from the other committee members attestation broadcasts
    pub node_index: NodeIndex,
    pub epoch: Epoch,
}

pub struct Execution<
    T: BroadcastEventInterface<PubSubMsg>,
    Q: SyncQueryRunnerInterface,
    NE: Emitter,
> {
    /// Managing certificates generated by narwhal.
    executor: ExecutionEngineSocket,
    /// Used to signal internal consensus processes that it is time to reconfigure for a new epoch
    reconfigure_notify: Arc<Notify>,
    /// Used to send payloads to the edge node consensus to broadcast out to other nodes
    tx_narwhal_batches: mpsc::Sender<(AuthenticStampedParcel, bool)>,
    /// Query runner to check application state, mainly used to make sure the last executed block
    /// is up to date from time we were an edge node
    query_runner: Q,
    /// Notifications emitter
    notifier: NE,
    /// Send the event to the RPC
    event_tx: OnceLock<mpsc::Sender<Vec<Event>>>,
    /// Stores the parcels and attestations.
    txn_store: RwLock<TransactionStore<T>>,
    /// For non-validators only: digests of parcels we have stored and executed
    executed_digests: RwLock<HashSet<Digest>>,
    /// For non-validators only: digests of parcels we have stored but not yet executed
    pending_digests: RwLock<HashSet<Digest>>,
    parcel_timeout_data: RwLock<ParcelTimeoutData>,
}

impl<T: BroadcastEventInterface<PubSubMsg>, Q: SyncQueryRunnerInterface, NE: Emitter>
    Execution<T, Q, NE>
{
    pub fn new(
        executor: ExecutionEngineSocket,
        reconfigure_notify: Arc<Notify>,
        tx_narwhal_batches: mpsc::Sender<(AuthenticStampedParcel, bool)>,
        query_runner: Q,
        notifier: NE,
    ) -> Self {
        Self {
            executor,
            reconfigure_notify,
            tx_narwhal_batches,
            query_runner,
            notifier,
            event_tx: OnceLock::new(),
            txn_store: RwLock::new(TransactionStore::default()),
            executed_digests: RwLock::new(HashSet::with_capacity(512)),
            pending_digests: RwLock::new(HashSet::with_capacity(512)),
            parcel_timeout_data: RwLock::new(ParcelTimeoutData {
                last_executed_timestamp: None,
                // TODO(matthias): do some napkin math for these initial estimates
                estimated_tbe: Duration::from_secs(30),
                deviation_tbe: Duration::from_secs(5),
            }),
        }
    }

    // Returns true if the epoch changed
    pub(crate) async fn submit_batch(
        &self,
        payload: Vec<Transaction>,
        digest: Digest,
        sub_dag_index: u64,
    ) -> bool {
        let transactions = payload
            .into_iter()
            .filter_map(|txn| {
                // Filter out transactions that wont serialize or have already been executed
                if let Ok(txn) = TransactionRequest::try_from(txn.as_ref()) {
                    if !self.query_runner.has_executed_digest(txn.hash()) {
                        Some(txn)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let block = Block {
            digest,
            sub_dag_index,
            transactions,
        };

        let archive_block = block.clone();

        // Unfailable
        let response = self.executor.run(block).await.unwrap();
        info!("Consensus submitted new block to application");

        match self.event_tx.get() {
            Some(tx) => {
                if let Err(e) = tx
                    .send(
                        response
                            .txn_receipts
                            .iter()
                            .filter_map(|r| r.event.clone())
                            .collect(),
                    )
                    .await
                {
                    error!("We could not send a message to the RPC: {e}");
                }
            },
            None => {
                error!("Once Cell not initialized, this is a bug");
            },
        }

        let change_epoch = response.change_epoch;
        self.notifier.new_block(archive_block, response);

        if change_epoch {
            let epoch_number = self.query_runner.get_current_epoch();
            let epoch_hash = self
                .query_runner
                .get_metadata(&Metadata::LastEpochHash)
                .expect("We should have a last epoch hash")
                .maybe_hash()
                .expect("We should have gotten a hash, this is a bug");

            self.notifier.epoch_changed(epoch_number, epoch_hash);
        }

        change_epoch
    }

    pub fn shutdown(&self) {
        self.executor.downgrade();
    }

    pub fn set_event_tx(&self, tx: mpsc::Sender<Vec<Event>>) {
        self.event_tx.set(tx).unwrap();
    }

    // These methods are only used for non-validators

    pub fn store_parcel(
        &self,
        parcel: AuthenticStampedParcel,
        originator: NodeIndex,
        message_digest: Option<BroadcastDigest>,
    ) -> Result<()> {
        if let Ok(mut txn_store) = self.txn_store.write() {
            txn_store.store_parcel(parcel, originator, message_digest);
            Ok(())
        } else {
            Err(anyhow!("Failed to acquire lock"))
        }
    }

    pub fn store_pending_parcel(
        &self,
        parcel: AuthenticStampedParcel,
        originator: NodeIndex,
        message_digest: Option<BroadcastDigest>,
        event: T,
    ) -> Result<()> {
        if let Ok(mut txn_store) = self.txn_store.write() {
            txn_store.store_pending_parcel(parcel, originator, message_digest, event);
            Ok(())
        } else {
            Err(anyhow!("Failed to acquire lock"))
        }
    }

    pub fn store_attestation(&self, digest: Digest, node_index: NodeIndex) -> Result<()> {
        if let Ok(mut txn_store) = self.txn_store.write() {
            txn_store.store_attestation(digest, node_index);
            Ok(())
        } else {
            Err(anyhow!("Failed to acquire lock"))
        }
    }

    pub fn store_pending_attestation(
        &self,
        digest: Digest,
        node_index: NodeIndex,
        event: T,
    ) -> Result<()> {
        if let Ok(mut txn_store) = self.txn_store.write() {
            txn_store.store_pending_attestation(digest, node_index, event);
            Ok(())
        } else {
            Err(anyhow!("Failed to acquire lock"))
        }
    }

    pub fn get_parcel_message_digest(&self, digest: &Digest) -> Option<BroadcastDigest> {
        self.txn_store
            .read()
            .unwrap()
            .get_parcel(digest)
            .and_then(|p| p.message_digest)
    }

    pub fn contains_parcel(&self, digest: &Digest) -> bool {
        self.txn_store.read().unwrap().get_parcel(digest).is_some()
    }

    pub fn change_epoch(&self, committee: &[NodeIndex]) {
        self.txn_store.write().unwrap().change_epoch(committee)
    }

    // Threshold should be 2f + 1 of the committee
    // Returns true if the epoch has changed
    pub async fn try_execute(&self, digest: Digest, threshold: usize) -> Result<bool, NotExecuted> {
        // get the current chain head
        let head = self.query_runner.get_last_block();
        let mut epoch_changed = match self.try_execute_internal(digest, threshold, head).await {
            Ok(epoch_changed) => epoch_changed,
            Err(NotExecuted::MissingAttestations(_)) => false,
            Err(e) => return Err(e),
        };

        let digests: Vec<Digest> = self
            .pending_digests
            .read()
            .unwrap()
            .iter()
            .copied()
            .collect();
        for digest in digests {
            let contains_pending = { self.pending_digests.read().unwrap().contains(&digest) };
            if contains_pending {
                // get the current chain head
                let head = self.query_runner.get_last_block();
                if let Ok(epoch_changed_) = self.try_execute_internal(digest, threshold, head).await
                {
                    epoch_changed = epoch_changed || epoch_changed_;
                }
            }
        }
        Ok(epoch_changed)
    }

    async fn try_execute_internal(
        &self,
        digest: Digest,
        threshold: usize,
        head: Digest,
    ) -> Result<bool, NotExecuted> {
        if self.pending_digests.read().unwrap().contains(&digest) {
            // we already executed this parcel
            return Ok(false);
        }
        let num_attestations = self
            .txn_store
            .read()
            .unwrap()
            .get_attestations(&digest)
            .map(|x| x.len());
        if let Some(num_attestations) = num_attestations {
            if num_attestations >= threshold {
                // if we should execute we need to make sure we can connect this to our transaction
                // chain
                return self.try_execute_chain(digest, head).await;
            }
        }
        Err(NotExecuted::MissingAttestations(digest))
    }

    async fn try_execute_chain(&self, digest: Digest, head: Digest) -> Result<bool, NotExecuted> {
        let mut txn_chain = VecDeque::new();
        let mut last_digest = digest;
        let mut parcel_chain = Vec::new();

        loop {
            let Some(parcel) = self
                .txn_store
                .read()
                .unwrap()
                .get_parcel(&last_digest)
                .cloned()
            else {
                break;
            };

            parcel_chain.push(last_digest);

            txn_chain.push_front((
                parcel.inner.transactions,
                parcel.inner.sub_dag_index,
                last_digest,
            ));

            if parcel.inner.last_executed == head {
                let mut epoch_changed = false;

                // We connected the chain now execute all the transactions
                for (batch, sub_dag_index, digest) in txn_chain {
                    if self.submit_batch(batch, digest, sub_dag_index).await {
                        epoch_changed = true;
                    }
                }

                // Note: instead of aqcuiring the write lock once at the top of the loop, we
                // aqcuire it for each iteration. We do this to avoid holding the write lock across
                // the await from `submit_batch`.
                // mark all parcels in chain as executed
                let mut pending_digests = self.pending_digests.write().unwrap();
                let mut executed_digests = self.executed_digests.write().unwrap();
                for digest in parcel_chain {
                    pending_digests.remove(&digest);
                    executed_digests.insert(digest);
                }

                // TODO(matthias): technically this call should be inside the for loop where we
                // call `submit_batch`, but I think this might bias the estimate to be too low.
                self.update_estimated_tbe();

                return Ok(epoch_changed);
            } else {
                last_digest = parcel.inner.last_executed;
            }
        }
        let mut pending_digests = self.pending_digests.write().unwrap();
        for digest in parcel_chain {
            pending_digests.insert(digest);
        }
        Err(NotExecuted::MissingParcel {
            digest: last_digest,
            timeout: self.get_parcel_timeout(),
        })
    }

    pub fn get_parcel_timeout(&self) -> Duration {
        // TODO(matthias): estimate time between parcel executions
        let data = self.parcel_timeout_data.read().unwrap();
        let mut timeout = 4 * data.deviation_tbe + data.estimated_tbe;
        timeout = timeout.max(MIN_TBE);
        timeout = timeout.min(MAX_TBE);
        timeout
    }

    // This method should be called whenever we execute a parcel.
    fn update_estimated_tbe(&self) {
        let mut data = self.parcel_timeout_data.write().unwrap();
        if let Some(timestamp) = data.last_executed_timestamp {
            if let Ok(sample_tbe) = timestamp.elapsed() {
                let sample_tbe = sample_tbe.as_millis() as f64;
                let estimated_tbe = data.estimated_tbe.as_millis() as f64;
                let new_estimated_tbe = (1.0 - TBE_EMA) * estimated_tbe + TBE_EMA * sample_tbe;
                data.estimated_tbe = Duration::from_millis(new_estimated_tbe as u64);

                let deviation_tbe = data.deviation_tbe.as_millis() as f64;
                let new_deviation_tbe = (1.0 - TBE_EMA) * deviation_tbe
                    + TBE_EMA * (new_estimated_tbe - sample_tbe).abs();
                data.deviation_tbe = Duration::from_millis(new_deviation_tbe as u64);
            }
        }
        data.last_executed_timestamp = Some(SystemTime::now());
    }
}

#[async_trait]
impl<T: BroadcastEventInterface<PubSubMsg>, Q: SyncQueryRunnerInterface, NE: Emitter> ExecutionState
    for Execution<T, Q, NE>
{
    async fn handle_consensus_output(&self, consensus_output: ConsensusOutput) {
        let current_epoch = self.query_runner.get_current_epoch();

        let sub_dag_index = consensus_output.sub_dag.sub_dag_index;

        let batch_payload: Vec<Vec<u8>> = consensus_output
            .batches
            .into_iter()
            .filter_map(|(cert, batch)| {
                // Skip over the ones that have a different epoch. Shouldnt ever happen besides an
                // edge case towards the end of an epoch
                if cert.epoch() != current_epoch {
                    error!("we recieved a consensus cert from an epoch we are not on");
                    None
                } else {
                    // Map the batch to just the transactions
                    Some(
                        batch
                            .into_iter()
                            .flat_map(|batch| match batch {
                                // work around because batch.transactions() would require clone
                                Batch::V1(btch) => btch.transactions,
                                Batch::V2(btch) => btch.transactions,
                            })
                            .collect::<Vec<Vec<u8>>>(),
                    )
                }
            })
            .flatten()
            .collect();

        if batch_payload.is_empty() {
            return;
        }
        // We have batches in the payload send them over broadcast along with an attestion
        // of them
        let last_executed = self.query_runner.get_last_block();
        let parcel = AuthenticStampedParcel {
            transactions: batch_payload.clone(),
            last_executed,
            epoch: current_epoch,
            sub_dag_index,
        };

        let epoch_changed = self
            .submit_batch(batch_payload, parcel.to_digest(), sub_dag_index)
            .await;

        if let Err(e) = self.tx_narwhal_batches.send((parcel, epoch_changed)).await {
            // This shouldn't ever happen. But if it does there is no critical tasks
            // happening on the other end of this that would require a
            // panic
            error!("Narwhal failed to send batch payload to edge consensus: {e:?}");
        }

        // Submit the batches to application layer and if the epoch changed reset last
        // executed
        if epoch_changed {
            self.reconfigure_notify.notify_waiters();
        }
    }

    async fn last_executed_sub_dag_index(&self) -> u64 {
        // Note we add one here because of an off by 1 error in Narwhal codebase
        // if we actually return the last sub dag index that we exectuted during a restart that is
        // going to be the sub dag index they send us after a restart and we will re-execute it
        self.query_runner.get_sub_dag_index() + 1
    }
}

struct ParcelTimeoutData {
    last_executed_timestamp: Option<SystemTime>,
    estimated_tbe: Duration,
    deviation_tbe: Duration,
}

#[derive(Debug)]
pub enum NotExecuted {
    MissingParcel { digest: Digest, timeout: Duration },
    MissingAttestations(Digest),
}
