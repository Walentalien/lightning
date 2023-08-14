use async_trait::async_trait;

use crate::{
    common::WithStartAndShutdown, config::ConfigConsumer, consensus::MempoolSocket,
    SyncQueryRunnerInterface,
};

/// The interface for the *RPC* server. Which is supposed to be opening a public
/// port (possibly an HTTP server) and accepts queries or updates from the user.
#[async_trait]
pub trait RpcInterface<Q: SyncQueryRunnerInterface>:
    Sized + Send + Sync + ConfigConsumer + WithStartAndShutdown
{
    /// Initialize the *RPC* server, with the given parameters.
    fn init(config: Self::Config, mempool: MempoolSocket, query_runner: Q) -> anyhow::Result<Self>;

    #[cfg(feature = "e2e-test")]
    fn provide_dht_socket(&self, dht_socket: crate::dht::DhtSocket);
}
