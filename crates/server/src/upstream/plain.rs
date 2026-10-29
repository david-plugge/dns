//! Plain DNS over UDP, retried over TCP when the response is truncated.

use std::net::SocketAddr;

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
};

use super::{TransportError, is_response_to};

/// Large enough for any EDNS payload size an upstream may use over UDP.
const UDP_RECEIVE_BUFFER: usize = 4096;

pub struct PlainTransport {
    address: SocketAddr,
}

impl PlainTransport {
    pub fn new(address: SocketAddr) -> Self {
        Self { address }
    }

    /// Sends `query` and returns the matching response. `query` must have a random ID.
    pub async fn exchange(&self, query: &Message) -> Result<Message, TransportError> {
        let wire_query = query.to_vec().map_err(TransportError::EncodeQuery)?;

        let response = self.exchange_udp(query, &wire_query).await?;
        if !response.metadata.truncation {
            return Ok(response);
        }
        self.exchange_tcp(query, &wire_query).await
    }

    async fn exchange_udp(
        &self,
        query: &Message,
        wire_query: &[u8],
    ) -> Result<Message, TransportError> {
        // A fresh socket per query gives every query a new random source port (RFC 5452).
        let socket = UdpSocket::bind(unspecified_address_like(self.address)).await?;
        socket.connect(self.address).await?;
        socket.send(wire_query).await?;

        let mut buffer = vec![0; UDP_RECEIVE_BUFFER];
        loop {
            let length = socket.recv(&mut buffer).await?;
            // Spoofed or late datagrams are dropped; we keep waiting for the real answer.
            match Message::from_vec(&buffer[..length]) {
                Ok(response) if is_response_to(&response, query) => return Ok(response),
                _ => continue,
            }
        }
    }

    async fn exchange_tcp(
        &self,
        query: &Message,
        wire_query: &[u8],
    ) -> Result<Message, TransportError> {
        let mut stream = TcpStream::connect(self.address).await?;

        // RFC 1035 4.2.2: each message is prefixed with its length as a big-endian u16.
        let length = u16::try_from(wire_query.len()).expect("encoded query exceeds 64 KiB");
        let mut framed = Vec::with_capacity(2 + wire_query.len());
        framed.extend_from_slice(&length.to_be_bytes());
        framed.extend_from_slice(wire_query);
        stream.write_all(&framed).await?;

        let length = stream.read_u16().await?;
        let mut buffer = vec![0; usize::from(length)];
        stream.read_exact(&mut buffer).await?;

        let response = Message::from_vec(&buffer)
            .map_err(|error| TransportError::InvalidResponse(error.to_string()))?;
        if !is_response_to(&response, query) {
            return Err(TransportError::MismatchedResponse);
        }
        Ok(response)
    }
}

fn unspecified_address_like(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::V6(_) => SocketAddr::from(([0u16; 8], 0)),
    }
}
