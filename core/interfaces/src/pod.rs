use affair::Socket;
use async_trait::async_trait;
use infusion::infu;

use crate::{
    common::WithStartAndShutdown, config::ConfigConsumer, infu_collection::Collection,
    signer::SubmitTxSocket, types::DeliveryAcknowledgment,
};

/// The socket which upon receiving a delivery acknowledgment can add it to the aggregator
/// queue which will later roll up a batch of delivery acknowledgments to the consensus.
pub type DeliveryAcknowledgmentSocket = Socket<DeliveryAcknowledgment, ()>;

#[async_trait]
pub trait DeliveryAcknowledgmentAggregatorInterface:
    WithStartAndShutdown + ConfigConsumer + Sized + Send + Sync
{
    infu!(DeliveryAcknowledgmentAggregatorInterface @ Input);

    /// Initialize a new delivery acknowledgment aggregator.
    fn init(config: Self::Config, submit_tx: SubmitTxSocket) -> anyhow::Result<Self>;

    /// Returns the socket that can be used to submit delivery acknowledgments to be aggregated.
    fn socket(&self) -> DeliveryAcknowledgmentSocket;
}

pub trait LaneManager {}
