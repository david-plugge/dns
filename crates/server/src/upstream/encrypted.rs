//! DNS over TLS and DNS over HTTPS on top of hickory-net's multiplexed connections.
//!
//! One connection per upstream is kept open and shared by all queries. It is opened
//! lazily and replaced after any error, so a dropped connection heals on the next query.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use hickory_net::{
    DnsHandle,
    h2::HttpsClientStream,
    runtime::TokioRuntimeProvider,
    tls::{client_config, tls_exchange},
    xfer::{DnsExchange, FirstAnswer},
};
use hickory_proto::op::{DnsRequest, DnsRequestOptions, Message};
use rustls_pki_types::ServerName;
use tokio::sync::Mutex;

use super::{TransportError, answers_question_of};

pub enum EncryptedProtocol {
    Tls,
    Https { path: Arc<str> },
}

pub struct EncryptedTransport {
    address: SocketAddr,
    server_name: ServerName<'static>,
    protocol: EncryptedProtocol,
    timeout: Duration,
    /// Owns the connections' background tasks; dropping it would abort them.
    runtime: TokioRuntimeProvider,
    connection: Mutex<Option<Connection>>,
    /// Numbers the opened connections, so a failed query only discards the one it used.
    opened: AtomicU64,
}

#[derive(Clone)]
struct Connection {
    exchange: DnsExchange<TokioRuntimeProvider>,
    generation: u64,
}

impl EncryptedTransport {
    pub fn new(
        address: SocketAddr,
        server_name: ServerName<'static>,
        protocol: EncryptedProtocol,
        timeout: Duration,
    ) -> Self {
        Self {
            address,
            server_name,
            protocol,
            timeout,
            runtime: TokioRuntimeProvider::new(),
            connection: Mutex::new(None),
            opened: AtomicU64::new(0),
        }
    }

    pub async fn exchange(&self, query: &Message) -> Result<Message, TransportError> {
        let (connection, reused) = self.connection().await?;
        match self.send(&connection, query).await {
            // Servers close idle connections, so a reused one may be dead without us knowing.
            Err(TransportError::Connection(_)) if reused => {
                let (connection, _) = self.connection().await?;
                self.send(&connection, query).await
            }
            result => result,
        }
    }

    async fn send(
        &self,
        connection: &Connection,
        query: &Message,
    ) -> Result<Message, TransportError> {
        let request = DnsRequest::new(query.clone(), DnsRequestOptions::default());
        match connection.exchange.send(request).first_answer().await {
            Ok(response) => {
                let response = response.into_message();
                // The multiplexer assigns its own message ID and matches responses by it.
                if !answers_question_of(&response, query) {
                    return Err(TransportError::MismatchedResponse);
                }
                Ok(response)
            }
            Err(error) => {
                let mut current = self.connection.lock().await;
                // Another query may already have replaced the failed connection.
                if current
                    .as_ref()
                    .is_some_and(|open| open.generation == connection.generation)
                {
                    *current = None;
                }
                Err(TransportError::Connection(error.to_string()))
            }
        }
    }

    /// Returns the open connection, or opens one, and whether it was already open.
    /// Holding the lock while connecting makes concurrent queries wait for a single
    /// handshake instead of starting their own.
    async fn connection(&self) -> Result<(Connection, bool), TransportError> {
        let mut connection = self.connection.lock().await;
        if let Some(open) = connection.as_ref() {
            return Ok((open.clone(), true));
        }
        let opened = Connection {
            exchange: self.connect().await?,
            generation: self.opened.fetch_add(1, Ordering::Relaxed),
        };
        *connection = Some(opened.clone());
        Ok((opened, false))
    }

    async fn connect(&self) -> Result<DnsExchange<TokioRuntimeProvider>, TransportError> {
        let tls_config =
            client_config().map_err(|error| TransportError::Connection(error.to_string()))?;
        let provider = self.runtime.clone();
        let connecting = async {
            match &self.protocol {
                EncryptedProtocol::Tls => tls_exchange(
                    self.address,
                    self.server_name.clone(),
                    tls_config,
                    self.timeout,
                    None,
                    provider,
                )
                .await
                .map_err(|error| TransportError::Connection(error.to_string())),
                EncryptedProtocol::Https { path } => {
                    let mut builder = HttpsClientStream::builder(Arc::new(tls_config), provider);
                    builder.request_timeout(self.timeout);
                    builder
                        .exchange(
                            self.address,
                            Arc::from(self.server_name.to_str()),
                            path.clone(),
                        )
                        .await
                        .map_err(|error| TransportError::Connection(error.to_string()))
                }
            }
        };
        tokio::time::timeout(self.timeout, connecting)
            .await
            .unwrap_or(Err(TransportError::Timeout))
    }
}
