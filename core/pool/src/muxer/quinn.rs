use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use fleek_crypto::{NodePublicKey, NodeSecretKey};
use quinn::{ClientConfig, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig};
use rustls::Certificate;

use crate::endpoint::NodeAddress;
use crate::muxer::{ConnectionInterface, MuxerInterface};
use crate::tls;

#[derive(Clone)]
pub struct Config {
    pub server_config: ServerConfig,
    pub address: SocketAddr,
    pub sk: NodeSecretKey,
}

#[derive(Clone)]
pub struct QuinnMuxer {
    endpoint: Endpoint,
    sk: NodeSecretKey,
}

#[async_trait]
impl MuxerInterface for QuinnMuxer {
    type Connecting = Connecting;
    type Connection = Connection;
    type Config = Config;

    fn init(config: Self::Config) -> io::Result<Self> {
        let endpoint = Endpoint::server(config.server_config, config.address)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        tracing::info!("bound to {:?}", endpoint.local_addr()?);

        Ok(Self {
            endpoint,
            sk: config.sk,
        })
    }

    async fn connect(&self, peer: NodeAddress, server_name: &str) -> io::Result<Self::Connecting> {
        let tls_config = tls::make_client_config(&self.sk, Some(peer.pk))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let mut client_config = ClientConfig::new(Arc::new(tls_config));
        let mut transport_config = TransportConfig::default();
        transport_config.max_idle_timeout(Some(Duration::from_secs(300).try_into().unwrap()));
        client_config.transport_config(Arc::new(transport_config));
        let connecting = self
            .endpoint
            .connect_with(client_config, peer.socket_address, server_name)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(Connecting(connecting))
    }

    async fn accept(&self) -> Option<Self::Connecting> {
        self.endpoint.accept().await.map(Connecting)
    }
}

pub struct Connecting(quinn::Connecting);

impl Future for Connecting {
    type Output = io::Result<Connection>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|result| result.map(Connection))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

#[derive(Clone)]
pub struct Connection(quinn::Connection);

#[async_trait]
impl ConnectionInterface for Connection {
    type SendStream = SendStream;
    type RecvStream = RecvStream;

    async fn open_stream(&mut self) -> io::Result<(Self::SendStream, Self::RecvStream)> {
        self.0
            .open_bi()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    async fn open_uni_stream(&mut self) -> io::Result<Self::SendStream> {
        self.0
            .open_uni()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    async fn accept_stream(&mut self) -> io::Result<(Self::SendStream, Self::RecvStream)> {
        self.0
            .accept_bi()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    async fn accept_uni_stream(&mut self) -> io::Result<Self::RecvStream> {
        self.0
            .accept_uni()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn peer_identity(&self) -> Option<NodePublicKey> {
        match self.0.peer_identity() {
            None => {
                tracing::error!("failed to get peer identity from successful TLS handshake");
                None
            },
            Some(any) => {
                let chain = match any.downcast::<Vec<Certificate>>() {
                    Ok(chain) => chain,
                    Err(e) => {
                        tracing::error!("invalid peer certificate: {e:?}");
                        return None;
                    },
                };
                let certificate = chain.first()?;
                match tls::parse_unverified(certificate.as_ref()) {
                    Ok(cert) => Some(cert.peer_pk()),
                    Err(e) => {
                        tracing::error!("failed to parse certificate {e:?}");
                        None
                    },
                }
            },
        }
    }

    fn remote_address(&self) -> SocketAddr {
        self.0.remote_address()
    }

    fn connection_id(&self) -> usize {
        self.0.stable_id()
    }

    fn close(&self, error_code: u8, reason: &[u8]) {
        self.0.close(error_code.into(), reason);
    }
}