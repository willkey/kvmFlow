use kvmflow_protocol::{
    EnvelopeReader, ProtocolEnvelope, ProtocolError, ProtocolMessage, write_envelope,
};
use tokio::net::TcpStream;
use tokio_rustls::TlsStream;

use crate::TrustedPeer;

pub type SecureReadHalf = tokio::io::ReadHalf<TlsStream<TcpStream>>;
pub type SecureWriteHalf = tokio::io::WriteHalf<TlsStream<TcpStream>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelTrust {
    Pairing { code: String },
    Authenticated,
}

pub struct ChannelParts {
    pub reader: SecureReadHalf,
    pub writer: SecureWriteHalf,
    pub session_id: String,
    pub peer: TrustedPeer,
    pub trust: ChannelTrust,
    pub next_sequence: u64,
    pub envelope_reader: EnvelopeReader,
}

impl std::fmt::Debug for ChannelParts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChannelParts")
            .field("session_id", &self.session_id)
            .field("peer", &self.peer)
            .field("trust", &self.trust)
            .field("next_sequence", &self.next_sequence)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct SecureChannel {
    stream: TlsStream<TcpStream>,
    session_id: String,
    next_sequence: u64,
    last_received_sequence: u64,
    envelope_reader: EnvelopeReader,
    peer: TrustedPeer,
    trust: ChannelTrust,
}

impl SecureChannel {
    pub(crate) fn new(
        stream: TlsStream<TcpStream>,
        session_id: String,
        peer: TrustedPeer,
        trust: ChannelTrust,
    ) -> Self {
        Self {
            stream,
            session_id,
            next_sequence: 1,
            last_received_sequence: 0,
            envelope_reader: EnvelopeReader::default(),
            peer,
            trust,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn peer(&self) -> &TrustedPeer {
        &self.peer
    }

    pub fn trust(&self) -> &ChannelTrust {
        &self.trust
    }

    pub async fn send(&mut self, message: ProtocolMessage) -> Result<(), ChannelError> {
        let envelope = ProtocolEnvelope::new(self.session_id.clone(), self.next_sequence, message);
        write_envelope(&mut self.stream, &envelope).await?;
        self.next_sequence = self.next_sequence.saturating_add(1);
        Ok(())
    }

    pub async fn receive(&mut self) -> Result<ProtocolMessage, ChannelError> {
        let envelope = self.envelope_reader.read_from(&mut self.stream).await?;
        if envelope.session_id != self.session_id {
            return Err(ChannelError::SessionMismatch);
        }
        if envelope.sequence <= self.last_received_sequence {
            return Err(ChannelError::NonMonotonicSequence {
                previous: self.last_received_sequence,
                received: envelope.sequence,
            });
        }
        self.last_received_sequence = envelope.sequence;
        Ok(envelope.message)
    }

    pub fn into_parts(self) -> ChannelParts {
        let (reader, writer) = tokio::io::split(self.stream);
        ChannelParts {
            reader,
            writer,
            session_id: self.session_id,
            peer: self.peer,
            trust: self.trust,
            next_sequence: self.next_sequence,
            envelope_reader: self.envelope_reader,
        }
    }

    pub(crate) fn into_raw_parts(self) -> (TlsStream<TcpStream>, Vec<u8>) {
        (self.stream, self.envelope_reader.into_buffered())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("received a frame for a different session")]
    SessionMismatch,
    #[error("received non-monotonic sequence {received} after {previous}")]
    NonMonotonicSequence { previous: u64, received: u64 },
}
