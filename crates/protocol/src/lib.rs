#![forbid(unsafe_code)]

//! Versioned wire messages and bounded framing for the KVM transport.

mod frame;
mod pairing;

pub use frame::{
    ChannelPurpose, ClipboardOffer, ClipboardStream, ClipboardStreamKind, EnvelopeReader,
    MAX_FRAME_BYTES, PROTOCOL_VERSION, ProtocolEnvelope, ProtocolError, ProtocolMessage,
    read_envelope, write_envelope,
};
pub use pairing::{PAIRING_CODE_DIGITS, derive_pairing_code};
