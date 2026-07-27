use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFile {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPayload {
    pub id: String,
    pub captured_at_unix_ms: u64,
    pub text: Option<String>,
    pub html: Option<String>,
    pub rich_text: Option<String>,
    pub image_png: Option<Vec<u8>>,
    pub files: Vec<ClipboardFile>,
}

impl ClipboardPayload {
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.html.is_none()
            && self.rich_text.is_none()
            && self.image_png.is_none()
            && self.files.is_empty()
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut hasher);
        self.html.hash(&mut hasher);
        self.rich_text.hash(&mut hasher);
        self.image_png.hash(&mut hasher);
        self.files.hash(&mut hasher);
        hasher.finish()
    }

    pub fn total_inline_bytes(&self) -> u64 {
        self.text.as_ref().map_or(0, |value| value.len() as u64)
            + self.html.as_ref().map_or(0, |value| value.len() as u64)
            + self
                .rich_text
                .as_ref()
                .map_or(0, |value| value.len() as u64)
            + self
                .image_png
                .as_ref()
                .map_or(0, |value| value.len() as u64)
    }

    pub fn kind(&self) -> ClipboardContentKind {
        let mut count = 0;
        let mut kind = ClipboardContentKind::Text;
        for (present, candidate) in [
            (self.text.is_some(), ClipboardContentKind::Text),
            (self.html.is_some(), ClipboardContentKind::Html),
            (self.rich_text.is_some(), ClipboardContentKind::RichText),
            (self.image_png.is_some(), ClipboardContentKind::Image),
            (!self.files.is_empty(), ClipboardContentKind::Files),
        ] {
            if present {
                count += 1;
                kind = candidate;
            }
        }
        if count > 1 {
            ClipboardContentKind::Mixed
        } else {
            kind
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardContentKind {
    Text,
    Html,
    RichText,
    Image,
    Files,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardHistoryItem {
    pub id: String,
    pub captured_at_unix_ms: u64,
    pub kind: ClipboardContentKind,
    pub preview: String,
    pub size_bytes: u64,
    pub available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardTransferState {
    Idle,
    Sending,
    Receiving,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardStatus {
    pub transfer_state: ClipboardTransferState,
    pub last_error: Option<String>,
    pub last_updated_unix_ms: Option<u64>,
    pub history: Vec<ClipboardHistoryItem>,
}

impl Default for ClipboardStatus {
    fn default() -> Self {
        Self {
            transfer_state: ClipboardTransferState::Idle,
            last_error: None,
            last_updated_unix_ms: None,
            history: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_fingerprint_ignores_transport_identity_and_timestamp() {
        let payload = ClipboardPayload {
            id: "first".into(),
            captured_at_unix_ms: 1,
            text: Some("hello".into()),
            html: None,
            rich_text: None,
            image_png: None,
            files: Vec::new(),
        };
        let mut same_content = payload.clone();
        same_content.id = "second".into();
        same_content.captured_at_unix_ms = 2;

        assert_eq!(payload.fingerprint(), same_content.fingerprint());
    }
}
