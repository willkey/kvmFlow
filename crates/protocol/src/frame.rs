use kvmflow_domain::{EndpointRole, InputEvent, ScreenEdge};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u16 = 2;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEnvelope {
    pub protocol_version: u16,
    pub session_id: String,
    pub sequence: u64,
    pub message: ProtocolMessage,
}

impl ProtocolEnvelope {
    pub fn new(session_id: impl Into<String>, sequence: u64, message: ProtocolMessage) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            session_id: session_id.into(),
            sequence,
            message,
        }
    }
}

#[derive(Debug, Default)]
pub struct EnvelopeReader {
    buffered: Vec<u8>,
}

impl EnvelopeReader {
    pub async fn read_from<R>(&mut self, reader: &mut R) -> Result<ProtocolEnvelope, ProtocolError>
    where
        R: AsyncRead + Unpin,
    {
        loop {
            if let Some(envelope) = self.decode_buffered()? {
                return Ok(envelope);
            }

            let mut chunk = [0_u8; READ_CHUNK_BYTES];
            let read = reader.read(&mut chunk).await?;
            if read == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
            }
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }

    fn decode_buffered(&mut self) -> Result<Option<ProtocolEnvelope>, ProtocolError> {
        let Some(length_bytes) = self.buffered.get(..4) else {
            return Ok(None);
        };
        let length = u32::from_be_bytes(
            length_bytes
                .try_into()
                .expect("four-byte frame length checked"),
        ) as usize;
        if length > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge(length));
        }
        let frame_length = 4 + length;
        if self.buffered.len() < frame_length {
            return Ok(None);
        }

        let envelope = deserialize_envelope(&self.buffered[4..frame_length])?;
        self.buffered.drain(..frame_length);
        Ok(Some(envelope))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ProtocolMessage {
    IdentityHello {
        role: EndpointRole,
        device_id: String,
        device_name: String,
        public_key: Vec<u8>,
        nonce: Vec<u8>,
        signature: Vec<u8>,
        paired_peer_id: Option<String>,
    },
    PairingDecision {
        confirmed: bool,
    },
    PairingProgress {
        remote_confirmed: bool,
    },
    SessionReady,
    Heartbeat {
        sent_at_unix_ms: u64,
    },
    HeartbeatAck {
        sent_at_unix_ms: u64,
    },
    EnterControl {
        edge: ScreenEdge,
        position: f32,
    },
    EnterControlAck {
        accepted: bool,
        reason: Option<String>,
    },
    ReturnControl {
        position: f32,
    },
    ReturnControlAck,
    Input {
        event: InputEvent,
    },
    ReleaseAll,
    RemotePause {
        paused: bool,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame length {0} exceeds the {MAX_FRAME_BYTES}-byte limit")]
    FrameTooLarge(usize),
    #[error("frame serialization failed: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("frame deserialization failed: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("unsupported protocol version {received}; expected {PROTOCOL_VERSION}")]
    VersionMismatch { received: u16 },
}

pub async fn write_envelope<W>(
    writer: &mut W,
    envelope: &ProtocolEnvelope,
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(envelope).map_err(ProtocolError::Serialize)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge(payload.len()));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| ProtocolError::FrameTooLarge(payload.len()))?;
    writer.write_u32(length).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_envelope<R>(reader: &mut R) -> Result<ProtocolEnvelope, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    EnvelopeReader::default().read_from(reader).await
}

fn deserialize_envelope(payload: &[u8]) -> Result<ProtocolEnvelope, ProtocolError> {
    let envelope: ProtocolEnvelope =
        serde_json::from_slice(payload).map_err(ProtocolError::Deserialize)?;
    if envelope.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch {
            received: envelope.protocol_version,
        });
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvmflow_domain::{KeyAction, KeyUsage};

    #[tokio::test]
    async fn envelope_round_trips_over_a_fragmentable_stream() {
        let (mut client, mut server) = tokio::io::duplex(32);
        let expected = ProtocolEnvelope::new(
            "session-1",
            7,
            ProtocolMessage::Input {
                event: InputEvent::Key {
                    usage: KeyUsage::A,
                    action: KeyAction::Down,
                },
            },
        );
        let sent = expected.clone();

        let writer = tokio::spawn(async move { write_envelope(&mut client, &sent).await });
        let received = read_envelope(&mut server).await.expect("read envelope");
        writer.await.expect("join writer").expect("write envelope");

        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn rejects_an_unsupported_protocol_version() {
        let (mut client, mut server) = tokio::io::duplex(128);
        let mut envelope = ProtocolEnvelope::new(
            "session-1",
            1,
            ProtocolMessage::ReturnControl { position: 0.5 },
        );
        envelope.protocol_version = PROTOCOL_VERSION + 1;
        let payload = serde_json::to_vec(&envelope).expect("serialize fixture");

        client
            .write_u32(payload.len() as u32)
            .await
            .expect("write length");
        client.write_all(&payload).await.expect("write payload");

        assert!(matches!(
            read_envelope(&mut server).await,
            Err(ProtocolError::VersionMismatch { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn stateful_reader_survives_cancellation_after_a_partial_frame() {
        use std::time::Duration;

        let expected = ProtocolEnvelope::new(
            "session-1",
            1,
            ProtocolMessage::ReturnControl { position: 0.5 },
        );
        let payload = serde_json::to_vec(&expected).expect("serialize fixture");
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&payload);
        let (mut writer, mut stream) = tokio::io::duplex(frame.len());
        let split_at = 8;
        let (partial_sent, partial_received) = tokio::sync::oneshot::channel();

        let sender = tokio::spawn(async move {
            writer
                .write_all(&frame[..split_at])
                .await
                .expect("write frame prefix and partial payload");
            partial_sent.send(()).expect("signal partial frame");
            tokio::time::sleep(Duration::from_secs(1)).await;
            writer
                .write_all(&frame[split_at..])
                .await
                .expect("write remaining frame");
        });
        partial_received.await.expect("wait for partial frame");

        let mut reader = EnvelopeReader::default();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), reader.read_from(&mut stream))
                .await
                .is_err()
        );
        assert_eq!(
            reader
                .read_from(&mut stream)
                .await
                .expect("resume frame read"),
            expected
        );
        sender.await.expect("join sender");
    }
}
