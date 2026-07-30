use std::str;

use kvmflow_protocol::ClipboardOffer;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsStream;

use crate::SecureChannel;

pub(crate) const BULK_CHUNK_BYTES: usize = 256 * 1024;
const MAX_BULK_FRAME_BYTES: usize = BULK_CHUNK_BYTES + 64 * 1024;
const READ_CHUNK_BYTES: usize = 32 * 1024;
const OFFER_FRAME: u8 = 1;
const CHUNK_FRAME: u8 = 2;
const COMPLETE_FRAME: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BulkMessage {
    Offer(ClipboardOffer),
    Chunk {
        transfer_id: String,
        stream_id: u32,
        offset: u64,
        data: Vec<u8>,
    },
    Complete {
        transfer_id: String,
    },
}

pub(crate) struct BulkChannel {
    stream: TlsStream<TcpStream>,
    buffered: Vec<u8>,
}

impl BulkChannel {
    pub(crate) fn from_secure(channel: SecureChannel) -> Self {
        let (stream, buffered) = channel.into_raw_parts();
        Self { stream, buffered }
    }

    pub(crate) async fn send(&mut self, message: &BulkMessage) -> Result<(), BulkProtocolError> {
        match message {
            BulkMessage::Offer(offer) => {
                let payload = serde_json::to_vec(offer)?;
                let frame_length = 1_usize.saturating_add(payload.len());
                validate_frame_length(frame_length)?;
                self.stream.write_u32(frame_length as u32).await?;
                self.stream.write_u8(OFFER_FRAME).await?;
                self.stream.write_all(&payload).await?;
            }
            BulkMessage::Chunk {
                transfer_id,
                stream_id,
                offset,
                data,
            } => {
                let transfer_id_length = validate_transfer_id_length(transfer_id)?;
                if data.len() > BULK_CHUNK_BYTES {
                    return Err(BulkProtocolError::ChunkTooLarge(data.len()));
                }
                let frame_length = 1 + 2 + transfer_id.len() + 4 + 8 + 4 + data.len();
                validate_frame_length(frame_length)?;
                self.stream.write_u32(frame_length as u32).await?;
                self.stream.write_u8(CHUNK_FRAME).await?;
                self.stream.write_u16(transfer_id_length).await?;
                self.stream.write_all(transfer_id.as_bytes()).await?;
                self.stream.write_u32(*stream_id).await?;
                self.stream.write_u64(*offset).await?;
                self.stream.write_u32(data.len() as u32).await?;
                self.stream.write_all(data).await?;
            }
            BulkMessage::Complete { transfer_id } => {
                let transfer_id_length = validate_transfer_id_length(transfer_id)?;
                let frame_length = 1 + 2 + transfer_id.len();
                validate_frame_length(frame_length)?;
                self.stream.write_u32(frame_length as u32).await?;
                self.stream.write_u8(COMPLETE_FRAME).await?;
                self.stream.write_u16(transfer_id_length).await?;
                self.stream.write_all(transfer_id.as_bytes()).await?;
            }
        }
        self.stream.flush().await?;
        Ok(())
    }

    pub(crate) async fn receive(&mut self) -> Result<BulkMessage, BulkProtocolError> {
        loop {
            if let Some(message) = take_buffered_message(&mut self.buffered)? {
                return Ok(message);
            }

            let mut chunk = [0_u8; READ_CHUNK_BYTES];
            let read = self.stream.read(&mut chunk).await?;
            if read == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
            }
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }
}

fn take_buffered_message(buffered: &mut Vec<u8>) -> Result<Option<BulkMessage>, BulkProtocolError> {
    let Some(length_bytes) = buffered.get(..4) else {
        return Ok(None);
    };
    let length = u32::from_be_bytes(
        length_bytes
            .try_into()
            .expect("four-byte frame length checked"),
    ) as usize;
    validate_frame_length(length)?;
    let frame_end = 4 + length;
    if buffered.len() < frame_end {
        return Ok(None);
    }
    let frame = buffered[4..frame_end].to_vec();
    buffered.drain(..frame_end);
    decode_message(frame).map(Some)
}

#[cfg(test)]
fn encode_message(message: &BulkMessage) -> Result<Vec<u8>, BulkProtocolError> {
    let mut frame = Vec::new();
    match message {
        BulkMessage::Offer(offer) => {
            frame.push(OFFER_FRAME);
            frame.extend(serde_json::to_vec(offer)?);
        }
        BulkMessage::Chunk {
            transfer_id,
            stream_id,
            offset,
            data,
        } => {
            if data.len() > BULK_CHUNK_BYTES {
                return Err(BulkProtocolError::ChunkTooLarge(data.len()));
            }
            frame.push(CHUNK_FRAME);
            encode_transfer_id(&mut frame, transfer_id)?;
            frame.extend(stream_id.to_be_bytes());
            frame.extend(offset.to_be_bytes());
            frame.extend((data.len() as u32).to_be_bytes());
            frame.extend(data);
        }
        BulkMessage::Complete { transfer_id } => {
            frame.push(COMPLETE_FRAME);
            encode_transfer_id(&mut frame, transfer_id)?;
        }
    }
    if frame.len() > MAX_BULK_FRAME_BYTES {
        return Err(BulkProtocolError::InvalidFrameLength(frame.len()));
    }
    Ok(frame)
}

fn decode_message(mut frame: Vec<u8>) -> Result<BulkMessage, BulkProtocolError> {
    let (&kind, payload) = frame
        .split_first()
        .ok_or(BulkProtocolError::InvalidFrameLength(0))?;
    match kind {
        OFFER_FRAME => Ok(BulkMessage::Offer(serde_json::from_slice(payload)?)),
        CHUNK_FRAME => {
            let (transfer_id, stream_id, offset, data_offset) = {
                let mut cursor = FrameCursor::new(payload);
                let transfer_id = cursor.read_transfer_id()?;
                let stream_id = cursor.read_u32()?;
                let offset = cursor.read_u64()?;
                let data_length = cursor.read_u32()? as usize;
                if data_length > BULK_CHUNK_BYTES || cursor.remaining() != data_length {
                    return Err(BulkProtocolError::InvalidChunkLength(data_length));
                }
                (transfer_id, stream_id, offset, 1 + cursor.offset)
            };
            Ok(BulkMessage::Chunk {
                transfer_id,
                stream_id,
                offset,
                data: frame.split_off(data_offset),
            })
        }
        COMPLETE_FRAME => {
            let mut cursor = FrameCursor::new(payload);
            let transfer_id = cursor.read_transfer_id()?;
            if cursor.remaining() != 0 {
                return Err(BulkProtocolError::TrailingBytes);
            }
            Ok(BulkMessage::Complete { transfer_id })
        }
        other => Err(BulkProtocolError::UnknownFrameKind(other)),
    }
}

#[cfg(test)]
fn encode_transfer_id(frame: &mut Vec<u8>, transfer_id: &str) -> Result<(), BulkProtocolError> {
    let length = validate_transfer_id_length(transfer_id)?;
    frame.extend(length.to_be_bytes());
    frame.extend(transfer_id.as_bytes());
    Ok(())
}

fn validate_transfer_id_length(transfer_id: &str) -> Result<u16, BulkProtocolError> {
    u16::try_from(transfer_id.len())
        .map_err(|_| BulkProtocolError::InvalidTransferIdLength(transfer_id.len()))
}

fn validate_frame_length(length: usize) -> Result<(), BulkProtocolError> {
    if length == 0 || length > MAX_BULK_FRAME_BYTES {
        Err(BulkProtocolError::InvalidFrameLength(length))
    } else {
        Ok(())
    }
}

struct FrameCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> FrameCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], BulkProtocolError> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(BulkProtocolError::TruncatedFrame)?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn read_u16(&mut self) -> Result<u16, BulkProtocolError> {
        Ok(u16::from_be_bytes(
            self.read_exact(2)?
                .try_into()
                .expect("two-byte slice checked"),
        ))
    }

    fn read_u32(&mut self) -> Result<u32, BulkProtocolError> {
        Ok(u32::from_be_bytes(
            self.read_exact(4)?
                .try_into()
                .expect("four-byte slice checked"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, BulkProtocolError> {
        Ok(u64::from_be_bytes(
            self.read_exact(8)?
                .try_into()
                .expect("eight-byte slice checked"),
        ))
    }

    fn read_transfer_id(&mut self) -> Result<String, BulkProtocolError> {
        let length = self.read_u16()? as usize;
        let value = str::from_utf8(self.read_exact(length)?)?;
        Ok(value.to_owned())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BulkProtocolError {
    #[error("bulk channel I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("bulk frame serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("bulk frame length {0} is invalid")]
    InvalidFrameLength(usize),
    #[error("bulk frame kind {0} is unknown")]
    UnknownFrameKind(u8),
    #[error("bulk frame is truncated")]
    TruncatedFrame,
    #[error("bulk frame contains trailing bytes")]
    TrailingBytes,
    #[error("bulk transfer id length {0} is invalid")]
    InvalidTransferIdLength(usize),
    #[error("bulk transfer id is not UTF-8: {0}")]
    InvalidTransferId(#[from] str::Utf8Error),
    #[error("bulk chunk length {0} is invalid")]
    InvalidChunkLength(usize),
    #[error("bulk chunk of {0} bytes exceeds its limit")]
    ChunkTooLarge(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvmflow_protocol::{ClipboardStream, ClipboardStreamKind};

    #[test]
    fn binary_messages_round_trip_without_base64_expansion() {
        let messages = [
            BulkMessage::Offer(ClipboardOffer {
                transfer_id: "transfer-1".into(),
                captured_at_unix_ms: 42,
                reset: true,
                manifest_complete: true,
                streams: vec![ClipboardStream {
                    id: 7,
                    kind: ClipboardStreamKind::File,
                    relative_path: Some("folder/file.bin".into()),
                    size_bytes: 4,
                }],
                directories: vec!["folder".into()],
                clipboard_roots: vec!["folder".into()],
            }),
            BulkMessage::Chunk {
                transfer_id: "transfer-1".into(),
                stream_id: 7,
                offset: 12,
                data: vec![0, 1, 254, 255],
            },
            BulkMessage::Complete {
                transfer_id: "transfer-1".into(),
            },
        ];

        for message in messages {
            let encoded = encode_message(&message).expect("encode");
            assert_eq!(decode_message(encoded).expect("decode"), message);
        }
    }

    #[test]
    fn oversized_and_truncated_chunks_are_rejected() {
        assert!(matches!(
            encode_message(&BulkMessage::Chunk {
                transfer_id: "transfer-1".into(),
                stream_id: 1,
                offset: 0,
                data: vec![0; BULK_CHUNK_BYTES + 1],
            }),
            Err(BulkProtocolError::ChunkTooLarge(_))
        ));

        let mut encoded = encode_message(&BulkMessage::Chunk {
            transfer_id: "transfer-1".into(),
            stream_id: 1,
            offset: 0,
            data: vec![1, 2, 3],
        })
        .expect("encode");
        encoded.pop();
        assert!(matches!(
            decode_message(encoded),
            Err(BulkProtocolError::InvalidChunkLength(3))
        ));
    }

    #[test]
    fn buffered_decoder_preserves_partial_frames_across_poll_cancellation() {
        let message = BulkMessage::Complete {
            transfer_id: "transfer-1".into(),
        };
        let frame = encode_message(&message).expect("encode");
        let mut wire = (frame.len() as u32).to_be_bytes().to_vec();
        wire.extend_from_slice(&frame);
        let mut buffered = wire[..4].to_vec();

        assert_eq!(
            take_buffered_message(&mut buffered).expect("partial length"),
            None
        );
        buffered.extend_from_slice(&wire[4..8]);
        assert_eq!(
            take_buffered_message(&mut buffered).expect("partial payload"),
            None
        );
        buffered.extend_from_slice(&wire[8..]);
        assert_eq!(
            take_buffered_message(&mut buffered).expect("complete frame"),
            Some(message)
        );
        assert!(buffered.is_empty());
    }

    #[test]
    fn buffered_decoder_keeps_the_next_complete_frame() {
        let first = BulkMessage::Complete {
            transfer_id: "first".into(),
        };
        let second = BulkMessage::Complete {
            transfer_id: "second".into(),
        };
        let mut buffered = Vec::new();
        for message in [&first, &second] {
            let frame = encode_message(message).expect("encode");
            buffered.extend_from_slice(&(frame.len() as u32).to_be_bytes());
            buffered.extend_from_slice(&frame);
        }

        assert_eq!(
            take_buffered_message(&mut buffered).expect("first frame"),
            Some(first)
        );
        assert_eq!(
            take_buffered_message(&mut buffered).expect("second frame"),
            Some(second)
        );
        assert!(buffered.is_empty());
    }
}
