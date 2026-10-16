use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use kvmflow_domain::{ClipboardFile, ClipboardPayload};
use kvmflow_protocol::{ClipboardOffer, ClipboardStream, ClipboardStreamKind};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
};

use crate::bulk::{BULK_CHUNK_BYTES, BulkMessage};

const MANIFEST_ENTRIES_PER_FRAME: usize = 32;
const MAX_RELATIVE_PATH_BYTES: usize = 512;

pub(crate) async fn prune_incoming_cache(
    data_dir: &Path,
    retained_transfer_ids: &HashSet<String>,
    keep_latest: usize,
) -> Result<(), std::io::Error> {
    let incoming_root = data_dir.join("clipboard-cache").join("incoming");
    let mut entries = match tokio::fs::read_dir(&incoming_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut candidates = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let Some(transfer_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if validate_transfer_id(&transfer_id).is_err() {
            continue;
        }
        let modified = entry
            .metadata()
            .await?
            .modified()
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        candidates.push((modified, transfer_id, entry.path()));
    }
    candidates.sort_by_key(|candidate| Reverse(candidate.0));
    let latest = candidates
        .iter()
        .take(keep_latest)
        .map(|(_, transfer_id, _)| transfer_id.clone())
        .collect::<HashSet<_>>();
    for (_, transfer_id, path) in candidates {
        if retained_transfer_ids.contains(&transfer_id) || latest.contains(&transfer_id) {
            continue;
        }
        tokio::fs::remove_dir_all(path).await?;
    }
    Ok(())
}

pub(crate) struct OutgoingClipboardTransfer {
    transfer_id: String,
    manifest: VecDeque<BulkMessage>,
    streams: Vec<OutgoingStream>,
    stream_index: usize,
    complete_sent: bool,
}

impl OutgoingClipboardTransfer {
    pub(crate) async fn prepare(payload: ClipboardPayload) -> Result<Self, ClipboardTransferError> {
        tokio::task::spawn_blocking(move || prepare_outgoing(payload))
            .await
            .map_err(|error| ClipboardTransferError::Preparation(error.to_string()))?
    }

    pub(crate) async fn next_message(
        &mut self,
    ) -> Result<Option<BulkMessage>, ClipboardTransferError> {
        if let Some(message) = self.manifest.pop_front() {
            return Ok(Some(message));
        }
        while let Some(stream) = self.streams.get_mut(self.stream_index) {
            if let Some(message) = stream.next_message(&self.transfer_id).await? {
                return Ok(Some(message));
            }
            self.stream_index += 1;
        }
        if self.complete_sent {
            return Ok(None);
        }
        self.complete_sent = true;
        Ok(Some(BulkMessage::Complete {
            transfer_id: self.transfer_id.clone(),
        }))
    }

    pub(crate) fn transfer_id(&self) -> &str {
        &self.transfer_id
    }
}

struct OutgoingStream {
    descriptor: ClipboardStream,
    source: OutgoingSource,
    offset: u64,
}

enum OutgoingSource {
    Memory(Vec<u8>),
    File { path: PathBuf, file: Option<File> },
}

impl OutgoingStream {
    async fn next_message(
        &mut self,
        transfer_id: &str,
    ) -> Result<Option<BulkMessage>, ClipboardTransferError> {
        if self.offset >= self.descriptor.size_bytes {
            return Ok(None);
        }
        let remaining = (self.descriptor.size_bytes - self.offset) as usize;
        let read_size = remaining.min(BULK_CHUNK_BYTES);
        let mut bytes = vec![0_u8; read_size];
        match &mut self.source {
            OutgoingSource::Memory(data) => {
                let start = self.offset as usize;
                bytes.copy_from_slice(&data[start..start + read_size]);
            }
            OutgoingSource::File { path, file } => {
                if file.is_none() {
                    *file = Some(File::open(path).await?);
                }
                read_file_chunk(file.as_mut().expect("clipboard file opened"), &mut bytes).await?;
            }
        }
        let offset = self.offset;
        self.offset += bytes.len() as u64;
        if self.offset == self.descriptor.size_bytes
            && let OutgoingSource::File { file, .. } = &mut self.source
        {
            *file = None;
        }
        Ok(Some(BulkMessage::Chunk {
            transfer_id: transfer_id.to_owned(),
            stream_id: self.descriptor.id,
            offset,
            data: bytes,
        }))
    }
}

async fn read_file_chunk(
    reader: &mut (impl AsyncRead + Unpin),
    bytes: &mut Vec<u8>,
) -> std::io::Result<()> {
    // This future is selected alongside incoming traffic. Commit a short read before
    // yielding so cancellation cannot advance the file without advancing the offset.
    let read = reader.read(bytes).await?;
    if read == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "clipboard source ended before its advertised size",
        ));
    }
    bytes.truncate(read);
    Ok(())
}

pub(crate) struct IncomingClipboardTransfer {
    transfer_id: String,
    captured_at_unix_ms: u64,
    root: PathBuf,
    streams: HashMap<u32, IncomingStream>,
    clipboard_roots: Vec<String>,
    manifest_complete: bool,
    active_file: Option<(u32, File)>,
}

impl IncomingClipboardTransfer {
    pub(crate) fn transfer_id(&self) -> &str {
        &self.transfer_id
    }

    pub(crate) async fn begin(
        data_dir: &Path,
        offer: ClipboardOffer,
    ) -> Result<Self, ClipboardTransferError> {
        validate_transfer_id(&offer.transfer_id)?;
        if !offer.reset {
            return Err(ClipboardTransferError::InvalidManifest(
                "first clipboard manifest frame must reset the transfer".into(),
            ));
        }
        let root = data_dir
            .join("clipboard-cache")
            .join("incoming")
            .join(&offer.transfer_id);
        if tokio::fs::try_exists(&root).await? {
            tokio::fs::remove_dir_all(&root).await?;
        }
        tokio::fs::create_dir_all(&root).await?;
        let mut incoming = Self {
            transfer_id: offer.transfer_id.clone(),
            captured_at_unix_ms: offer.captured_at_unix_ms,
            root,
            streams: HashMap::new(),
            clipboard_roots: Vec::new(),
            manifest_complete: false,
            active_file: None,
        };
        incoming.append_offer(offer).await?;
        Ok(incoming)
    }

    pub(crate) async fn append_offer(
        &mut self,
        offer: ClipboardOffer,
    ) -> Result<(), ClipboardTransferError> {
        if offer.transfer_id != self.transfer_id || offer.reset && !self.streams.is_empty() {
            return Err(ClipboardTransferError::InvalidManifest(
                "clipboard manifest transfer identity changed".into(),
            ));
        }
        if self.manifest_complete {
            return Err(ClipboardTransferError::InvalidManifest(
                "clipboard manifest was already completed".into(),
            ));
        }
        for relative in offer.directories {
            let relative = safe_relative_path(&relative)?;
            tokio::fs::create_dir_all(self.root.join(relative)).await?;
        }
        for root in offer.clipboard_roots {
            let relative = safe_relative_path(&root)?;
            self.clipboard_roots
                .push(relative.to_string_lossy().into_owned());
        }
        for descriptor in offer.streams {
            if self.streams.contains_key(&descriptor.id) {
                return Err(ClipboardTransferError::InvalidManifest(format!(
                    "duplicate clipboard stream {}",
                    descriptor.id
                )));
            }
            let path = stream_destination(&self.root, &descriptor)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            drop(File::create(&path).await?);
            self.streams.insert(
                descriptor.id,
                IncomingStream {
                    descriptor,
                    path,
                    received: 0,
                },
            );
        }
        self.manifest_complete = offer.manifest_complete;
        Ok(())
    }

    pub(crate) async fn write_chunk(
        &mut self,
        stream_id: u32,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ClipboardTransferError> {
        if !self.manifest_complete {
            return Err(ClipboardTransferError::InvalidManifest(
                "clipboard data arrived before the manifest completed".into(),
            ));
        }
        if data.len() > BULK_CHUNK_BYTES {
            return Err(ClipboardTransferError::InvalidChunk(
                "clipboard chunk exceeds the bounded chunk size".into(),
            ));
        }
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| ClipboardTransferError::InvalidChunk("unknown stream".into()))?;
        if offset != stream.received
            || stream.received.saturating_add(data.len() as u64) > stream.descriptor.size_bytes
        {
            return Err(ClipboardTransferError::InvalidChunk(
                "clipboard chunk offset or size is invalid".into(),
            ));
        }
        if self
            .active_file
            .as_ref()
            .is_none_or(|(id, _)| *id != stream_id)
        {
            if let Some((_, mut file)) = self.active_file.take() {
                file.flush().await?;
            }
            self.active_file = Some((
                stream_id,
                File::options().append(true).open(&stream.path).await?,
            ));
        }
        let (_, file) = self
            .active_file
            .as_mut()
            .expect("active clipboard file opened");
        file.write_all(data).await?;
        stream.received += data.len() as u64;
        if stream.received == stream.descriptor.size_bytes {
            file.flush().await?;
            self.active_file = None;
        }
        Ok(())
    }

    pub(crate) async fn complete(self) -> Result<ClipboardPayload, ClipboardTransferError> {
        if !self.manifest_complete
            || self
                .streams
                .values()
                .any(|stream| stream.received != stream.descriptor.size_bytes)
        {
            return Err(ClipboardTransferError::Incomplete);
        }
        let mut payload = ClipboardPayload {
            id: self.transfer_id,
            captured_at_unix_ms: self.captured_at_unix_ms,
            text: None,
            html: None,
            rich_text: None,
            image_png: None,
            files: self
                .clipboard_roots
                .iter()
                .map(|relative| ClipboardFile {
                    name: Path::new(relative)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("file")
                        .to_owned(),
                    path: self.root.join(relative),
                })
                .collect(),
        };
        for stream in self.streams.into_values() {
            match stream.descriptor.kind {
                ClipboardStreamKind::Text => {
                    payload.text = Some(tokio::fs::read_to_string(stream.path).await?)
                }
                ClipboardStreamKind::Html => {
                    payload.html = Some(tokio::fs::read_to_string(stream.path).await?)
                }
                ClipboardStreamKind::RichText => {
                    payload.rich_text = Some(tokio::fs::read_to_string(stream.path).await?)
                }
                ClipboardStreamKind::ImagePng => {
                    payload.image_png = Some(tokio::fs::read(stream.path).await?)
                }
                ClipboardStreamKind::File => {}
            }
        }
        Ok(payload)
    }
}

struct IncomingStream {
    descriptor: ClipboardStream,
    path: PathBuf,
    received: u64,
}

fn prepare_outgoing(
    payload: ClipboardPayload,
) -> Result<OutgoingClipboardTransfer, ClipboardTransferError> {
    validate_transfer_id(&payload.id)?;
    let mut streams = Vec::new();
    let mut directories = Vec::new();
    let mut clipboard_roots = Vec::new();
    let mut next_stream_id = 0_u32;
    for (kind, data) in [
        (
            ClipboardStreamKind::Text,
            payload.text.map(String::into_bytes),
        ),
        (
            ClipboardStreamKind::Html,
            payload.html.map(String::into_bytes),
        ),
        (
            ClipboardStreamKind::RichText,
            payload.rich_text.map(String::into_bytes),
        ),
        (ClipboardStreamKind::ImagePng, payload.image_png),
    ] {
        if let Some(data) = data {
            let descriptor = ClipboardStream {
                id: next_stream_id,
                kind,
                relative_path: None,
                size_bytes: data.len() as u64,
            };
            streams.push(OutgoingStream {
                descriptor,
                source: OutgoingSource::Memory(data),
                offset: 0,
            });
            next_stream_id += 1;
        }
    }
    let mut used_roots = HashSet::new();
    for file in payload.files {
        let root_name = unique_root_name(&file.name, &mut used_roots);
        clipboard_roots.push(root_name.clone());
        collect_path(
            &file.path,
            Path::new(&root_name),
            &mut next_stream_id,
            &mut streams,
            &mut directories,
        )?;
    }
    let descriptors: Vec<_> = streams
        .iter()
        .map(|stream| stream.descriptor.clone())
        .collect();
    let manifest = build_manifest_frames(
        &payload.id,
        payload.captured_at_unix_ms,
        descriptors,
        directories,
        clipboard_roots,
    );
    Ok(OutgoingClipboardTransfer {
        transfer_id: payload.id,
        manifest,
        streams,
        stream_index: 0,
        complete_sent: false,
    })
}

fn collect_path(
    source: &Path,
    relative: &Path,
    next_stream_id: &mut u32,
    streams: &mut Vec<OutgoingStream>,
    directories: &mut Vec<String>,
) -> Result<(), ClipboardTransferError> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(ClipboardTransferError::InvalidSource(format!(
            "symbolic links are not transferred: {}",
            source.display()
        )));
    }
    validate_relative_path(relative)?;
    if metadata.is_dir() {
        directories.push(wire_relative_path(relative));
        let mut entries: Vec<_> = fs::read_dir(source)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let child_relative = relative.join(name);
            collect_path(
                &entry.path(),
                &child_relative,
                next_stream_id,
                streams,
                directories,
            )?;
        }
    } else if metadata.is_file() {
        let descriptor = ClipboardStream {
            id: *next_stream_id,
            kind: ClipboardStreamKind::File,
            relative_path: Some(wire_relative_path(relative)),
            size_bytes: metadata.len(),
        };
        streams.push(OutgoingStream {
            descriptor,
            source: OutgoingSource::File {
                path: source.to_owned(),
                file: None,
            },
            offset: 0,
        });
        *next_stream_id = next_stream_id
            .checked_add(1)
            .ok_or_else(|| ClipboardTransferError::InvalidManifest("too many streams".into()))?;
    }
    Ok(())
}

fn build_manifest_frames(
    transfer_id: &str,
    captured_at_unix_ms: u64,
    streams: Vec<ClipboardStream>,
    directories: Vec<String>,
    clipboard_roots: Vec<String>,
) -> VecDeque<BulkMessage> {
    let mut stream_chunks = streams.chunks(MANIFEST_ENTRIES_PER_FRAME);
    let mut directory_chunks = directories.chunks(MANIFEST_ENTRIES_PER_FRAME);
    let mut messages = VecDeque::new();
    let mut reset = true;
    loop {
        let stream_chunk = stream_chunks.next().unwrap_or_default().to_vec();
        let directory_chunk = directory_chunks.next().unwrap_or_default().to_vec();
        let more = stream_chunks.len() > 0 || directory_chunks.len() > 0;
        messages.push_back(BulkMessage::Offer(ClipboardOffer {
            transfer_id: transfer_id.to_owned(),
            captured_at_unix_ms,
            reset,
            manifest_complete: !more,
            streams: stream_chunk,
            directories: directory_chunk,
            clipboard_roots: if reset {
                clipboard_roots.clone()
            } else {
                Vec::new()
            },
        }));
        reset = false;
        if !more {
            break;
        }
    }
    messages
}

fn stream_destination(
    root: &Path,
    descriptor: &ClipboardStream,
) -> Result<PathBuf, ClipboardTransferError> {
    match descriptor.kind {
        ClipboardStreamKind::File => descriptor
            .relative_path
            .as_deref()
            .ok_or_else(|| {
                ClipboardTransferError::InvalidManifest("file stream has no relative path".into())
            })
            .and_then(safe_relative_path)
            .map(|relative| root.join(relative)),
        _ if descriptor.relative_path.is_none() => {
            Ok(root.join(".streams").join(descriptor.id.to_string()))
        }
        _ => Err(ClipboardTransferError::InvalidManifest(
            "inline clipboard stream has a path".into(),
        )),
    }
}

fn unique_root_name(name: &str, used: &mut HashSet<String>) -> String {
    let base = sanitize_component(name);
    if used.insert(base.clone()) {
        return base;
    }
    let mut sequence = 2;
    loop {
        let candidate = format!("{base} ({sequence})");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        sequence += 1;
    }
}

fn sanitize_component(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character => character,
        })
        .collect();
    let value = value.trim().trim_matches('.').trim();
    if value.is_empty() || value == "." || value == ".." {
        "file".into()
    } else {
        value.chars().take(128).collect()
    }
}

fn wire_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn safe_relative_path(value: &str) -> Result<PathBuf, ClipboardTransferError> {
    let path = Path::new(value);
    validate_relative_path(path)?;
    Ok(path.to_owned())
}

fn validate_relative_path(path: &Path) -> Result<(), ClipboardTransferError> {
    if path.to_string_lossy().len() > MAX_RELATIVE_PATH_BYTES
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ClipboardTransferError::InvalidManifest(format!(
            "unsafe clipboard path: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_transfer_id(transfer_id: &str) -> Result<(), ClipboardTransferError> {
    if transfer_id.is_empty()
        || transfer_id.len() > 128
        || !transfer_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(ClipboardTransferError::InvalidManifest(
            "invalid clipboard transfer id".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClipboardTransferError {
    #[error("clipboard transfer preparation failed: {0}")]
    Preparation(String),
    #[error("invalid clipboard source: {0}")]
    InvalidSource(String),
    #[error("invalid clipboard manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid clipboard chunk: {0}")]
    InvalidChunk(String),
    #[error("clipboard transfer is incomplete")]
    Incomplete,
    #[error("clipboard transfer I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_short_file_read_is_committed_before_the_worker_selects_another_branch() {
        let (mut source, mut reader) = tokio::io::duplex(16);
        source.write_all(b"abc").await.unwrap();
        let mut bytes = vec![0; 8];
        tokio::select! {
            biased;
            result = read_file_chunk(&mut reader, &mut bytes) => result.unwrap(),
            _ = std::future::ready(()) => panic!("partial input consumed without committing a chunk"),
        }
        assert_eq!(bytes, b"abc");
        source.write_all(b"defgh").await.unwrap();
        let mut rest = vec![0; 8];
        read_file_chunk(&mut reader, &mut rest).await.unwrap();
        assert_eq!([bytes, rest].concat(), b"abcdefgh");
    }

    #[tokio::test]
    async fn cancelling_a_pending_file_chunk_does_not_lose_bytes() {
        let (mut source, mut reader) = tokio::io::duplex(16);
        let mut bytes = vec![0; 4];
        tokio::select! {
            biased;
            _ = read_file_chunk(&mut reader, &mut bytes) => panic!("reader should wait for input"),
            _ = std::future::ready(()) => {},
        }
        source.write_all(b"data").await.unwrap();
        read_file_chunk(&mut reader, &mut bytes).await.unwrap();
        assert_eq!(bytes, b"data");
        drop(source);
        assert_eq!(
            read_file_chunk(&mut reader, &mut vec![0; 1])
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn unsafe_relative_paths_are_rejected() {
        assert!(safe_relative_path("../secret").is_err());
        assert!(safe_relative_path("/absolute").is_err());
        assert!(safe_relative_path("folder/file.txt").is_ok());
    }

    #[tokio::test]
    async fn text_payload_is_chunked_and_reassembled() {
        let payload = ClipboardPayload {
            id: "clipboard-1".into(),
            captured_at_unix_ms: 42,
            text: Some("x".repeat(BULK_CHUNK_BYTES + 5)),
            html: None,
            rich_text: None,
            image_png: None,
            files: Vec::new(),
        };
        let directory = tempfile::tempdir().expect("temp directory");
        let mut outgoing = OutgoingClipboardTransfer::prepare(payload.clone())
            .await
            .expect("prepare");
        let BulkMessage::Offer(offer) = outgoing
            .next_message()
            .await
            .expect("message")
            .expect("offer")
        else {
            panic!("expected offer");
        };
        let mut incoming = IncomingClipboardTransfer::begin(directory.path(), offer)
            .await
            .expect("begin");
        let mut chunk_sizes = Vec::new();
        while let Some(message) = outgoing.next_message().await.expect("message") {
            match message {
                BulkMessage::Chunk {
                    stream_id,
                    offset,
                    data,
                    ..
                } => {
                    chunk_sizes.push(data.len());
                    incoming
                        .write_chunk(stream_id, offset, &data)
                        .await
                        .expect("write");
                }
                BulkMessage::Complete { .. } => break,
                _ => panic!("unexpected message"),
            }
        }
        let completed = incoming.complete().await.expect("complete");
        assert_eq!(completed.text, payload.text);
        assert!(chunk_sizes.len() > 1);
        assert!(chunk_sizes.iter().all(|size| *size <= BULK_CHUNK_BYTES));
    }

    #[tokio::test]
    async fn large_directories_transfer_without_accumulating_open_files() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        for index in 0..600 {
            fs::write(
                source.path().join(format!("{index}.txt")),
                if index % 2 == 0 {
                    b"data".as_slice()
                } else {
                    b"".as_slice()
                },
            )
            .unwrap();
        }
        let payload = ClipboardPayload {
            id: "many-files".into(),
            captured_at_unix_ms: 1,
            text: None,
            html: None,
            rich_text: None,
            image_png: None,
            files: vec![ClipboardFile {
                name: "folder".into(),
                path: source.path().into(),
            }],
        };
        let mut outgoing = OutgoingClipboardTransfer::prepare(payload).await.unwrap();
        let mut incoming = None;
        while let Some(message) = outgoing.next_message().await.unwrap() {
            match message {
                BulkMessage::Offer(offer) if offer.reset => {
                    incoming = Some(
                        IncomingClipboardTransfer::begin(destination.path(), offer)
                            .await
                            .unwrap(),
                    );
                }
                BulkMessage::Offer(offer) => incoming
                    .as_mut()
                    .unwrap()
                    .append_offer(offer)
                    .await
                    .unwrap(),
                BulkMessage::Chunk {
                    stream_id,
                    offset,
                    data,
                    ..
                } => {
                    incoming
                        .as_mut()
                        .unwrap()
                        .write_chunk(stream_id, offset, &data)
                        .await
                        .unwrap();
                }
                BulkMessage::Complete { .. } => break,
            }
            assert!(
                outgoing
                    .streams
                    .iter()
                    .filter(|s| matches!(&s.source, OutgoingSource::File { file: Some(_), .. }))
                    .count()
                    <= 1
            );
        }
        let completed = incoming.unwrap().complete().await.unwrap();
        for index in 0..600 {
            assert_eq!(
                fs::read(completed.files[0].path.join(format!("{index}.txt"))).unwrap(),
                fs::read(source.path().join(format!("{index}.txt"))).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn incoming_cache_pruning_keeps_referenced_and_latest_transfers() {
        let directory = tempfile::tempdir().expect("temp directory");
        let incoming = directory.path().join("clipboard-cache").join("incoming");
        for transfer_id in ["transfer-1", "transfer-2", "transfer-3"] {
            let root = incoming.join(transfer_id);
            tokio::fs::create_dir_all(&root)
                .await
                .expect("create transfer");
            tokio::fs::write(root.join("payload"), transfer_id)
                .await
                .expect("write transfer");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        prune_incoming_cache(
            directory.path(),
            &HashSet::from(["transfer-1".to_owned()]),
            1,
        )
        .await
        .expect("prune cache");

        assert!(
            tokio::fs::try_exists(incoming.join("transfer-1"))
                .await
                .unwrap()
        );
        assert!(
            !tokio::fs::try_exists(incoming.join("transfer-2"))
                .await
                .unwrap()
        );
        assert!(
            tokio::fs::try_exists(incoming.join("transfer-3"))
                .await
                .unwrap()
        );
    }
}
