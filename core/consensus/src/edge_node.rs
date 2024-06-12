use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use fleek_crypto::NodePublicKey;
use lightning_interfaces::prelude::*;
use lightning_interfaces::types::{Epoch, NodeIndex};
use lightning_metrics::increment_counter;
use lightning_utils::application::QueryRunnerExt;
use quick_cache::unsync::Cache;
use tokio::pin;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::consensus::PubSubMsg;
use crate::execution::{AuthenticStampedParcel, CommitteeAttestation, Digest};
use crate::transaction_manager::{NotExecuted, TxnStoreCmd};

const MAX_PENDING_TIMEOUTS: usize = 100;
/// TODO(matthias): the txn store tracks the time between executions to get a better
/// estimate for this timeout. However, now we don't have access to the txn store anymore.
/// We could add another command to the txn store to get the timeout.
const PARCEL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct EdgeConsensus {
    handle: JoinHandle<()>,
    tx_shutdown: Arc<Notify>,
}

impl EdgeConsensus {
    pub fn spawn<
        C: Collection,
        P: PubSub<PubSubMsg> + 'static,
        Q: SyncQueryRunnerInterface,
        NE: Emitter,
    >(
        pub_sub: P,
        query_runner: Q,
        node_public_key: NodePublicKey,
        txn_mgr_cmd_tx: mpsc::Sender<TxnStoreCmd<P::Event>>,
        notifier: C::NotifierInterface,
        reconfigure_notify: Arc<Notify>,
    ) -> Self {
        let shutdown_notify = Arc::new(Notify::new());

        let handle = spawn!(
            message_receiver_worker::<C, P, Q>(
                pub_sub,
                shutdown_notify.clone(),
                query_runner,
                node_public_key,
                notifier,
                txn_mgr_cmd_tx,
                reconfigure_notify,
            ),
            "CONSENSUS: message receiver worker"
        );

        Self {
            handle,
            tx_shutdown: shutdown_notify,
        }
    }

    /// Consume this executor and shutdown all of the workers and processes.
    pub async fn shutdown(self) {
        // Send the shutdown signal.
        self.tx_shutdown.notify_one();

        // Gracefully wait for all the subtasks to finish and return.
        if let Err(e) = self.handle.await {
            error!(
                "Failed to join handle in file {} at line {}: {e}",
                file!(),
                line!()
            );
        }
    }
}

/// Creates and event loop which consumes messages from pubsub and sends them to the
/// right destination.
#[allow(clippy::too_many_arguments)]
async fn message_receiver_worker<
    C: Collection,
    P: PubSub<PubSubMsg>,
    Q: SyncQueryRunnerInterface,
>(
    mut pub_sub: P,
    shutdown_notify: Arc<Notify>,
    query_runner: Q,
    node_public_key: NodePublicKey,
    notifier: C::NotifierInterface,
    txn_mgr_cmd_tx: mpsc::Sender<TxnStoreCmd<P::Event>>,
    reconfigure_notify: Arc<Notify>,
) {
    info!("Edge node message worker is running");
    let mut committee = query_runner.get_committee_members_by_index();
    let mut quorom_threshold = (committee.len() * 2) / 3 + 1;
    let mut our_index = query_runner
        .pubkey_to_index(&node_public_key)
        .unwrap_or(u32::MAX);
    let mut on_committee = committee.contains(&our_index);
    let (timeout_tx, mut timeout_rx) = mpsc::channel(128);
    // `pending_timeouts` is not a cache because we already limit the number of timeouts we spawn
    // with `MAX_PENDING_TIMEOUTS`, so `pending_timeouts` is bounded from above by that constant
    let mut pending_timeouts = HashSet::new();
    let mut pending_requests = Cache::new(100);

    let mut epoch_changed_sub = notifier.subscribe_epoch_changed();

    let shutdown_future = shutdown_notify.notified();
    pin!(shutdown_future);
    loop {
        // todo(dalton): revisit pinning these and using Notify over oneshot

        tokio::select! {
            biased;
            _ = &mut shutdown_future => {
                break;
            },
            Some(_) = epoch_changed_sub.recv() => {
                // Note: only validators need to execute this branch because edge nodes
                // will know that the epoch changed, see below.
                // Maybe we remove that part from below and have both validators and edge nodes
                // execute this branch?
                if on_committee {
                    committee = query_runner.get_committee_members_by_index();
                    quorom_threshold = (committee.len() * 2) / 3 + 1;
                    // We recheck our index incase it was non existant before
                    // and we staked during this epoch and finally got the certificate
                    our_index = query_runner
                        .pubkey_to_index(&node_public_key)
                        .unwrap_or(u32::MAX);
                    on_committee = committee.contains(&our_index);
                }
            }
            Some(msg) = pub_sub.recv_event() => {
                handle_pubsub_event::<P, Q>(
                    msg,
                    &mut quorom_threshold,
                    &mut committee,
                    &mut our_index,
                    &mut on_committee,
                    &node_public_key,
                    &txn_mgr_cmd_tx,
                    &mut pending_timeouts,
                    &mut pending_requests,
                    &query_runner,
                    &pub_sub,
                    &timeout_tx,
                    &reconfigure_notify,
                ).await;
            },
            digest = timeout_rx.recv() => {
                // Timeout for a missing parcel. If we still haven't received the parcel, we send a
                // request.
                if let Some(digest) = digest {
                    pending_timeouts.remove(&digest);

                    let (response_tx, response_rx) = oneshot::channel();
                    txn_mgr_cmd_tx.send(TxnStoreCmd::ContainsParcel {
                        digest,
                        response: response_tx
                    }).await.expect("Failed to send get parcel msg digest command");
                    let contains_parcel = response_rx.await.expect("Failed to receive response from contains parcel command");

                    if !contains_parcel {
                        let request = PubSubMsg::RequestTransactions(digest);
                        let _ = pub_sub.send(&request, None).await;
                        pending_requests.insert(digest, ());
                        info!("Send request for missing parcel with digest: {digest:?}");

                        increment_counter!(
                            "consensus_missing_parcel_request",
                            Some("Counter for the number of times the node sent a request for a missing consensus parcel")
                        );
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_pubsub_event<P: PubSub<PubSubMsg>, Q: SyncQueryRunnerInterface>(
    mut msg: P::Event,
    quorom_threshold: &mut usize,
    committee: &mut Vec<NodeIndex>,
    our_index: &mut NodeIndex,
    on_committee: &mut bool,
    node_public_key: &NodePublicKey,
    txn_mgr_cmd_tx: &mpsc::Sender<TxnStoreCmd<P::Event>>,
    pending_timeouts: &mut HashSet<Digest>,
    pending_requests: &mut Cache<Digest, ()>,
    query_runner: &Q,
    pub_sub: &P,
    timeout_tx: &mpsc::Sender<Digest>,
    reconfigure_notify: &Arc<Notify>,
) {
    match msg.take().unwrap() {
        PubSubMsg::Transactions(parcel) => {
            handle_parcel::<P, Q>(
                msg,
                parcel,
                quorom_threshold,
                committee,
                our_index,
                on_committee,
                node_public_key,
                txn_mgr_cmd_tx,
                pending_timeouts,
                pending_requests,
                query_runner,
                timeout_tx,
                reconfigure_notify,
            )
            .await;
        },
        PubSubMsg::Attestation(att) => {
            handle_attestation::<P, Q>(
                msg,
                att,
                quorom_threshold,
                committee,
                our_index,
                on_committee,
                node_public_key,
                txn_mgr_cmd_tx,
                pending_timeouts,
                query_runner,
                timeout_tx,
                reconfigure_notify,
            )
            .await;
        },
        PubSubMsg::RequestTransactions(digest) => {
            let (response_tx, response_rx) = oneshot::channel();
            txn_mgr_cmd_tx
                .send(TxnStoreCmd::GetParcelMessageDigest {
                    digest,
                    response: response_tx,
                })
                .await
                .expect("Failed to send get parcel msg digest command");
            let parcel_msg_digest = response_rx
                .await
                .expect("Failed to receive response from get parcel msg digest command");
            if let Some(msg_digest) = parcel_msg_digest {
                let filter = HashSet::from([msg.originator()]);
                pub_sub.repropagate(msg_digest, Some(filter)).await;
                info!("Responded to request for missing parcel with digest: {digest:?}");
                increment_counter!(
                    "consensus_missing_parcel_sent",
                    Some("Number of missing parcels served to other nodes"),
                );
            } else {
                increment_counter!(
                    "consensus_missing_parcel_ignored",
                    Some(
                        "Number of parcel requests that were ignored due to not finding it in the transaction store"
                    ),
                );
            }
        },
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_parcel<P: PubSub<PubSubMsg>, Q: SyncQueryRunnerInterface>(
    msg: P::Event,
    parcel: AuthenticStampedParcel,
    quorom_threshold: &mut usize,
    committee: &mut Vec<NodeIndex>,
    our_index: &mut NodeIndex,
    on_committee: &mut bool,
    node_public_key: &NodePublicKey,
    txn_mgr_cmd_tx: &mpsc::Sender<TxnStoreCmd<P::Event>>,
    pending_timeouts: &mut HashSet<Digest>,
    pending_requests: &mut Cache<Digest, ()>,
    query_runner: &Q,
    timeout_tx: &mpsc::Sender<Digest>,
    reconfigure_notify: &Arc<Notify>,
) {
    let epoch = query_runner.get_current_epoch();
    let originator = msg.originator();
    let is_committee = committee.contains(&originator);
    if !is_valid_message(is_committee, parcel.epoch, epoch) {
        msg.mark_invalid_sender();
        return;
    }

    let msg_digest = msg.get_digest();
    let parcel_digest = parcel.to_digest();
    let from_next_epoch = parcel.epoch == epoch + 1;
    let last_executed = parcel.last_executed;

    let mut event = None;
    if pending_requests.remove(&parcel_digest).is_none() && !from_next_epoch {
        // We only want to propagate parcels that we did not request and that
        // are not from the next epoch.
        msg.propagate();
    } else {
        event = Some(msg);
    }

    if from_next_epoch {
        // if the parcel is from the next epoch, we optimistically
        // store it and check if it's from a validator once we change epochs.

        // Note: this unwrap is safe. If `from_next_epoch=true`, then `event`
        // will always be `Some`. Unfortunately, the borrow checker cannot
        // figure this out on its own if we use `msg` directly here.

        // TODO(matthias): panic here or only log the error?
        txn_mgr_cmd_tx
            .send(TxnStoreCmd::StorePendingParcel {
                parcel,
                originator,
                message_digest: msg_digest,
                event: event.unwrap(),
            })
            .await
            .expect("Failed to send pending parcel to txn store");
    } else {
        // only propagate normal parcels, not parcels we requested, and not
        // parcels that we optimistically accept from the next epoch
        txn_mgr_cmd_tx
            .send(TxnStoreCmd::StoreParcel {
                parcel,
                originator,
                message_digest: Some(msg_digest),
            })
            .await
            .expect("Failed to send parcel to txn store");
    }

    // Check if we requested this parcel
    if pending_requests.remove(&parcel_digest).is_some() {
        // This is a parcel that we specifically requested, so
        // we have to set a timeout for the previous parcel, because
        // swallow the Err return in the loop of `try_execute`.
        set_parcel_timer(
            last_executed,
            //txn_store.get_timeout(),
            PARCEL_TIMEOUT,
            timeout_tx.clone(),
            pending_timeouts,
        );
        info!("Received requested parcel with digest: {parcel_digest:?}");

        increment_counter!(
            "consensus_missing_parcel_received",
            Some("Number of missing parcels successfully received from other nodes")
        );
    }

    if !*on_committee {
        info!("Received transaction parcel from gossip as an edge node");

        try_execute::<P, Q>(
            parcel_digest,
            quorom_threshold,
            committee,
            our_index,
            on_committee,
            node_public_key,
            txn_mgr_cmd_tx,
            pending_timeouts,
            query_runner,
            timeout_tx,
            reconfigure_notify,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_attestation<P: PubSub<PubSubMsg>, Q: SyncQueryRunnerInterface>(
    msg: P::Event,
    att: CommitteeAttestation,
    quorom_threshold: &mut usize,
    committee: &mut Vec<NodeIndex>,
    our_index: &mut NodeIndex,
    on_committee: &mut bool,
    node_public_key: &NodePublicKey,
    txn_mgr_cmd_tx: &mpsc::Sender<TxnStoreCmd<P::Event>>,
    pending_timeouts: &mut HashSet<Digest>,
    query_runner: &Q,
    timeout_tx: &mpsc::Sender<Digest>,
    reconfigure_notify: &Arc<Notify>,
) {
    let originator = msg.originator();

    let epoch = query_runner.get_current_epoch();
    let is_committee = committee.contains(&originator);
    if originator != att.node_index || !is_valid_message(is_committee, att.epoch, epoch) {
        msg.mark_invalid_sender();
        return;
    }

    let from_next_epoch = att.epoch == epoch + 1;
    let mut event = None;
    if !from_next_epoch {
        msg.propagate();
    } else {
        event = Some(msg);
    }

    if !*on_committee {
        info!("Received parcel attestation from gossip as an edge node");

        if from_next_epoch {
            // if the attestation is from the next epoch, we optimistically
            // store it and check if it's from a validator once we change epochs
            // Note: this unwrap is safe. If `from_next_epoch=true`, then `event`
            // will always be `Some`. Unfortunately, the borrow checker cannot
            // figure this out on its own if we use `msg` directly here.
            txn_mgr_cmd_tx
                .send(TxnStoreCmd::StorePendingAttestation {
                    digest: att.digest,
                    node_index: att.node_index,
                    event: event.unwrap(),
                })
                .await
                .expect("Failed to send pending attestation to txn store");
        } else {
            txn_mgr_cmd_tx
                .send(TxnStoreCmd::StoreAttestation {
                    digest: att.digest,
                    node_index: att.node_index,
                })
                .await
                .expect("Failed to send attestation to txn store");
        }

        try_execute::<P, Q>(
            att.digest,
            quorom_threshold,
            committee,
            our_index,
            on_committee,
            node_public_key,
            txn_mgr_cmd_tx,
            pending_timeouts,
            query_runner,
            timeout_tx,
            reconfigure_notify,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn try_execute<P: PubSub<PubSubMsg>, Q: SyncQueryRunnerInterface>(
    digest: Digest,
    quorom_threshold: &mut usize,
    committee: &mut Vec<NodeIndex>,
    our_index: &mut NodeIndex,
    on_committee: &mut bool,
    node_public_key: &NodePublicKey,
    txn_mgr_cmd_tx: &mpsc::Sender<TxnStoreCmd<P::Event>>,
    pending_timeouts: &mut HashSet<Digest>,
    query_runner: &Q,
    timeout_tx: &mpsc::Sender<Digest>,
    reconfigure_notify: &Arc<Notify>,
) {
    let (response_tx, response_rx) = oneshot::channel();
    txn_mgr_cmd_tx
        .send(TxnStoreCmd::TryExecute {
            digest,
            quorom_threshold: *quorom_threshold,
            response: response_tx,
        })
        .await
        .expect("Failed to send try execute command");
    let res = response_rx
        .await
        .expect("Failed to receive response from try execute command");

    match res {
        Ok(epoch_changed) => {
            if epoch_changed {
                *committee = query_runner.get_committee_members_by_index();
                *quorom_threshold = (committee.len() * 2) / 3 + 1;
                // We recheck our index incase it was non existant before and
                // we staked during this epoch and finally got the certificate
                *our_index = query_runner
                    .pubkey_to_index(node_public_key)
                    .unwrap_or(u32::MAX);
                *on_committee = committee.contains(our_index);
                reconfigure_notify.notify_waiters();

                // Check the validity of the parcels/attestations that we
                // stored optimistically.
            }
        },
        Err(not_executed) => {
            handle_not_executed(not_executed, timeout_tx.clone(), pending_timeouts);
        },
    }
}

// While trying to connect the chain back to the head, we discovered a missing parcel.
// This is can happen normally, because parcels or attestations might arrive out of
// order.
// However, this could also mean that we missed a broadcast message containing a parcel,
// and our peers are no longer broadcasting this message.
// In this case we want to send a request out for this parcel.
// In order to prevent sending out these requests prematurely, we keep a running average of
// the intervals between executing parcels.
// If we are missing a parcel, and the time that has passed since trying toexecute the last parcel
// is larger than the expected time, we send out a request.
fn handle_not_executed(
    not_executed: NotExecuted,
    timeout_tx: mpsc::Sender<Digest>,
    pending_timeouts: &mut HashSet<Digest>,
) {
    if let NotExecuted::MissingParcel { digest, timeout: _ } = not_executed {
        set_parcel_timer(digest, PARCEL_TIMEOUT, timeout_tx, pending_timeouts);
    }
}

fn set_parcel_timer(
    digest: Digest,
    timeout: Duration,
    timeout_tx: mpsc::Sender<Digest>,
    pending_timeouts: &mut HashSet<Digest>,
) {
    if !pending_timeouts.contains(&digest) && pending_timeouts.len() < MAX_PENDING_TIMEOUTS {
        spawn!(
            async move {
                tokio::time::sleep(timeout).await;
                let _ = timeout_tx.send(digest).await;
            },
            "CONSENSUS: parcel timer"
        );
        pending_timeouts.insert(digest);
    }
}

// The parcel must be either from the current committee or from the
// next epoch. We optimistically accept parcels from the following
// epoch in case we missed the epoch change parcel. We will verify
// later on that the parcel was sent from a committee member.
fn is_valid_message(in_committee: bool, msg_epoch: Epoch, current_epoch: Epoch) -> bool {
    (in_committee && msg_epoch == current_epoch) || msg_epoch == current_epoch + 1
}

#[cfg(test)]
mod tests {
    use crate::edge_node::is_valid_message;

    #[test]
    fn test_is_valid_message() {
        // msg is from a committee member, msg epoch is the current epoch => valid
        assert!(is_valid_message(true, 2, 2));
        // msg is from a committee member, msg epoch is from the next epoch => valid
        assert!(is_valid_message(true, 4, 3));
        // msg is from a committee member, msg epoch is from the current epoch + 2 => invalid
        assert!(!is_valid_message(true, 7, 5));
        // msg is from a committee member, msg epoch is from the last epoch => invalid
        assert!(!is_valid_message(true, 4, 5));
        // msg is not from a committee member, msg epoch is the next epoch => valid
        assert!(is_valid_message(false, 2, 1));
        // msg is not from a committee member, msg epoch is the current epoch + 2 => invalid
        assert!(!is_valid_message(false, 3, 1));
        // msg is not from a committee member, msg epoch is the last epoch => invalid
        assert!(!is_valid_message(false, 1, 2));
    }
}
