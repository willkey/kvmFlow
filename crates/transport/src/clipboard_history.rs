use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use kvmflow_domain::{ClipboardHistoryItem, ClipboardPayload};
use serde::{Deserialize, Serialize};
use tempfile::Builder;
use tokio::io::AsyncWriteExt;

use crate::clipboard_transfer::prune_incoming_cache;

const HISTORY_LIMIT: usize = 20;

#[derive(Clone)]
pub(crate) struct ClipboardHistoryStore {
    data_dir: PathBuf,
    root: PathBuf,
    index_path: PathBuf,
}

impl ClipboardHistoryStore {
    pub(crate) fn new(data_dir: &Path) -> Self {
        let root = data_dir.join("clipboard-history");
        Self {
            data_dir: data_dir.to_owned(),
            index_path: root.join("index.json"),
            root,
        }
    }

    pub(crate) async fn summaries(
        &self,
    ) -> Result<Vec<ClipboardHistoryItem>, ClipboardHistoryError> {
        let index = self.load_index().await?;
        let mut summaries = Vec::new();
        for id in index.ids {
            if let Ok(payload) = self.load(&id).await {
                summaries.push(summary(&payload));
            }
        }
        Ok(summaries)
    }

    pub(crate) async fn record(
        &self,
        payload: &ClipboardPayload,
    ) -> Result<Vec<ClipboardHistoryItem>, ClipboardHistoryError> {
        validate_id(&payload.id)?;
        tokio::fs::create_dir_all(&self.root).await?;
        let mut index = self.load_index().await?;
        if index.ids.iter().any(|id| id == &payload.id) {
            return self.summaries().await;
        }
        let item_root = self.root.join(&payload.id);
        tokio::fs::create_dir_all(&item_root).await?;
        let mut stored_payload = payload.clone();
        let has_image = stored_payload.image_png.is_some();
        if let Some(image) = stored_payload.image_png.take() {
            atomic_write(&item_root.join("image.png"), &image).await?;
        }
        let metadata = StoredClipboard {
            payload: stored_payload,
            has_image,
        };
        atomic_write(
            &item_root.join("payload.json"),
            &serde_json::to_vec_pretty(&metadata)?,
        )
        .await?;

        index.ids.insert(0, payload.id.clone());
        while index.ids.len() > HISTORY_LIMIT {
            if let Some(evicted) = index.ids.pop() {
                let _ = tokio::fs::remove_dir_all(self.root.join(evicted)).await;
            }
        }
        self.save_index(&index).await?;
        self.prune_incoming_cache().await?;
        self.summaries().await
    }

    pub(crate) async fn load(&self, id: &str) -> Result<ClipboardPayload, ClipboardHistoryError> {
        validate_id(id)?;
        let item_root = self.root.join(id);
        let metadata: StoredClipboard =
            serde_json::from_slice(&tokio::fs::read(item_root.join("payload.json")).await?)?;
        let mut payload = metadata.payload;
        if metadata.has_image {
            payload.image_png = Some(tokio::fs::read(item_root.join("image.png")).await?);
        }
        Ok(payload)
    }

    pub(crate) async fn clear(&self) -> Result<(), ClipboardHistoryError> {
        if tokio::fs::try_exists(&self.root).await? {
            tokio::fs::remove_dir_all(&self.root).await?;
        }
        self.prune_incoming_cache().await?;
        Ok(())
    }

    pub(crate) async fn prune_incoming_cache(&self) -> Result<(), ClipboardHistoryError> {
        let index = self.load_index().await?;
        let incoming_root = self.data_dir.join("clipboard-cache").join("incoming");
        let mut retained_transfer_ids = HashSet::new();
        for id in index.ids {
            let Ok(payload) = self.load(&id).await else {
                continue;
            };
            for file in payload.files {
                let Ok(relative) = file.path.strip_prefix(&incoming_root) else {
                    continue;
                };
                if let Some(transfer_id) = relative
                    .components()
                    .next()
                    .and_then(|component| component.as_os_str().to_str())
                {
                    retained_transfer_ids.insert(transfer_id.to_owned());
                }
            }
        }
        prune_incoming_cache(&self.data_dir, &retained_transfer_ids, 1).await?;
        Ok(())
    }

    async fn load_index(&self) -> Result<HistoryIndex, ClipboardHistoryError> {
        match tokio::fs::read(&self.index_path).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(HistoryIndex::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn save_index(&self, index: &HistoryIndex) -> Result<(), ClipboardHistoryError> {
        tokio::fs::create_dir_all(&self.root).await?;
        atomic_write(&self.index_path, &serde_json::to_vec_pretty(index)?).await
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryIndex {
    ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredClipboard {
    payload: ClipboardPayload,
    has_image: bool,
}

fn summary(payload: &ClipboardPayload) -> ClipboardHistoryItem {
    ClipboardHistoryItem {
        id: payload.id.clone(),
        captured_at_unix_ms: payload.captured_at_unix_ms,
        kind: payload.kind(),
        preview: preview(payload),
        size_bytes: payload.total_inline_bytes()
            + payload
                .files
                .iter()
                .map(|file| path_size(&file.path))
                .sum::<u64>(),
        available: payload.files.iter().all(|file| file.path.exists()),
    }
}

fn preview(payload: &ClipboardPayload) -> String {
    if let Some(text) = payload.text.as_ref() {
        let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
        return compact.chars().take(80).collect();
    }
    if !payload.files.is_empty() {
        let mut names = payload
            .files
            .iter()
            .take(3)
            .map(|file| file.name.as_str())
            .collect::<Vec<_>>()
            .join("、");
        if payload.files.len() > 3 {
            names.push_str(" 等");
        }
        return names;
    }
    if payload.image_png.is_some() {
        return "PNG 图片".into();
    }
    if payload.html.is_some() {
        return "HTML 内容".into();
    }
    if payload.rich_text.is_some() {
        return "富文本内容".into();
    }
    "剪贴板内容".into()
}

fn path_size(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.file_type().is_symlink() && metadata.is_file() {
        metadata.len()
    } else {
        0
    }
}

async fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), ClipboardHistoryError> {
    let parent = path.parent().ok_or(ClipboardHistoryError::MissingParent)?;
    tokio::fs::create_dir_all(parent).await?;
    let temporary = Builder::new()
        .prefix("clipboard-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    let temporary_path = temporary.into_temp_path();
    let mut file = tokio::fs::File::create(&temporary_path).await?;
    file.write_all(contents).await?;
    file.flush().await?;
    file.sync_all().await?;
    temporary_path
        .persist(path)
        .map_err(|error| ClipboardHistoryError::Io(error.error))?;
    Ok(())
}

pub(crate) fn activated_payload_id() -> String {
    format!(
        "history-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}

fn validate_id(id: &str) -> Result<(), ClipboardHistoryError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(ClipboardHistoryError::InvalidId);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClipboardHistoryError {
    #[error("clipboard history I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("clipboard history serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("clipboard history path has no parent")]
    MissingParent,
    #[error("clipboard history id is invalid")]
    InvalidId,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(id: usize) -> ClipboardPayload {
        ClipboardPayload {
            id: format!("item-{id}"),
            captured_at_unix_ms: id as u64,
            text: Some(format!("history item {id}")),
            html: None,
            rich_text: None,
            image_png: None,
            files: Vec::new(),
        }
    }

    #[tokio::test]
    async fn history_persists_latest_twenty_and_can_be_cleared() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = ClipboardHistoryStore::new(directory.path());
        for id in 0..22 {
            store.record(&payload(id)).await.expect("record history");
        }

        let summaries = store.summaries().await.expect("load summaries");
        assert_eq!(summaries.len(), 20);
        assert_eq!(summaries[0].id, "item-21");
        assert_eq!(summaries[19].id, "item-2");
        assert_eq!(
            store.load("item-21").await.expect("load payload").text,
            Some("history item 21".into())
        );

        store.clear().await.expect("clear");
        assert!(store.summaries().await.expect("empty").is_empty());
    }

    #[tokio::test]
    async fn history_cache_pruning_preserves_referenced_file_roots() {
        let directory = tempfile::tempdir().expect("temp directory");
        let incoming = directory.path().join("clipboard-cache").join("incoming");
        let retained_root = incoming.join("remote-1");
        let stale_root = incoming.join("remote-2");
        tokio::fs::create_dir_all(&retained_root)
            .await
            .expect("create retained root");
        tokio::fs::write(retained_root.join("retained.txt"), "retained")
            .await
            .expect("write retained file");
        tokio::fs::create_dir_all(&stale_root)
            .await
            .expect("create stale root");
        tokio::fs::write(stale_root.join("stale.txt"), "stale")
            .await
            .expect("write stale file");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let latest_root = incoming.join("remote-3");
        tokio::fs::create_dir_all(&latest_root)
            .await
            .expect("create latest root");

        let store = ClipboardHistoryStore::new(directory.path());
        let mut retained_payload = payload(1);
        retained_payload.files.push(kvmflow_domain::ClipboardFile {
            name: "retained.txt".into(),
            path: retained_root.join("retained.txt"),
        });
        store
            .record(&retained_payload)
            .await
            .expect("record retained payload");

        assert!(tokio::fs::try_exists(retained_root).await.unwrap());
        assert!(!tokio::fs::try_exists(stale_root).await.unwrap());
        assert!(tokio::fs::try_exists(latest_root).await.unwrap());
    }
}
