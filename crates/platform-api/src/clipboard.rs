use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use clipboard_rs::{
    Clipboard, ClipboardContent, ClipboardContext, ClipboardHandler, ClipboardWatcher,
    ClipboardWatcherContext, ContentFormat, WatcherShutdown, common::RustImage,
};
use kvmflow_domain::{ClipboardFile, ClipboardPayload};
use tokio::sync::mpsc::Sender;

use crate::PlatformEvent;

const REMOTE_APPLY_SUPPRESSION: Duration = Duration::from_secs(2);

pub struct ClipboardAdapter {
    writer: ClipboardWriterHandle,
    watcher: ClipboardWatcherHandle,
}

impl ClipboardAdapter {
    pub fn start(events: Sender<PlatformEvent>) -> Result<Self, ClipboardAdapterError> {
        let suppression = Arc::new(Mutex::new(None));
        let mut watcher = ClipboardWatcherContext::new_with_interval(Duration::from_millis(500))
            .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
        let initial = capture_clipboard();
        let handler = ChangeHandler {
            events: events.clone(),
            suppression: suppression.clone(),
        };
        watcher.add_handler(handler);
        let (writer_tx, writer_rx) = mpsc::channel();
        let writer_suppression = suppression.clone();
        let writer_events = events.clone();
        let writer_join = std::thread::Builder::new()
            .name("kvmflow-clipboard-writer".into())
            .spawn(move || {
                while let Ok(payload) = writer_rx.recv() {
                    if let Err(error) = apply_clipboard(&writer_suppression, &payload) {
                        let _ = writer_events
                            .try_send(PlatformEvent::ClipboardFailed(error.to_string()));
                    }
                }
            })
            .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
        let writer = ClipboardWriterHandle {
            sender: Some(writer_tx),
            join: Some(writer_join),
        };
        let shutdown = watcher.get_shutdown_channel();
        let join = std::thread::Builder::new()
            .name("kvmflow-clipboard".into())
            .spawn(move || watcher.start_watch())
            .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
        match initial {
            Ok(Some(payload)) => {
                let _ = events.try_send(PlatformEvent::ClipboardChanged(payload));
            }
            Ok(None) => {}
            Err(error) => {
                let _ = events.try_send(PlatformEvent::ClipboardFailed(error.to_string()));
            }
        }
        Ok(Self {
            writer,
            watcher: ClipboardWatcherHandle {
                shutdown: Some(shutdown),
                join: Some(join),
            },
        })
    }

    pub fn apply(&self, payload: ClipboardPayload) -> Result<(), ClipboardAdapterError> {
        if payload.is_empty() {
            return Ok(());
        }
        self.writer
            .sender
            .as_ref()
            .ok_or(ClipboardAdapterError::WriterStopped)?
            .send(payload)
            .map_err(|_| ClipboardAdapterError::WriterStopped)
    }

    pub fn is_running(&self) -> bool {
        self.watcher.join.is_some()
    }
}

struct ClipboardWriterHandle {
    sender: Option<mpsc::Sender<ClipboardPayload>>,
    join: Option<JoinHandle<()>>,
}

impl Drop for ClipboardWriterHandle {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct ClipboardWatcherHandle {
    shutdown: Option<WatcherShutdown>,
    join: Option<JoinHandle<()>>,
}

impl Drop for ClipboardWatcherHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.stop();
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct ChangeHandler {
    events: Sender<PlatformEvent>,
    suppression: Arc<Mutex<Option<SuppressedClipboard>>>,
}

impl ClipboardHandler for ChangeHandler {
    fn on_clipboard_change(&mut self) {
        match capture_clipboard() {
            Ok(Some(payload)) => {
                let suppressed = self
                    .suppression
                    .lock()
                    .ok()
                    .and_then(|suppression| suppression.as_ref().copied())
                    .is_some_and(|suppression| {
                        let elapsed = suppression.applied_at.elapsed();
                        elapsed <= Duration::from_millis(250)
                            || elapsed <= REMOTE_APPLY_SUPPRESSION
                                && suppression.fingerprint == payload.fingerprint()
                    });
                if !suppressed {
                    let _ = self
                        .events
                        .blocking_send(PlatformEvent::ClipboardChanged(payload));
                }
            }
            Ok(None) => {}
            Err(error) => {
                let _ = self
                    .events
                    .blocking_send(PlatformEvent::ClipboardFailed(error.to_string()));
            }
        }
    }
}

#[derive(Clone, Copy)]
struct SuppressedClipboard {
    fingerprint: u64,
    applied_at: std::time::Instant,
}

fn apply_clipboard(
    suppression: &Arc<Mutex<Option<SuppressedClipboard>>>,
    payload: &ClipboardPayload,
) -> Result<(), ClipboardAdapterError> {
    let context = ClipboardContext::new()
        .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
    let mut contents = Vec::new();
    if let Some(text) = payload.text.clone() {
        contents.push(ClipboardContent::Text(text));
    }
    if let Some(html) = payload.html.clone() {
        contents.push(ClipboardContent::Html(html));
    }
    if let Some(rich_text) = payload.rich_text.clone() {
        contents.push(ClipboardContent::Rtf(rich_text));
    }
    if let Some(image_png) = payload.image_png.as_ref() {
        let image = clipboard_rs::RustImageData::from_bytes(image_png)
            .map_err(|error| ClipboardAdapterError::InvalidContent(error.to_string()))?;
        contents.push(ClipboardContent::Image(image));
    }
    if !payload.files.is_empty() {
        contents.push(ClipboardContent::Files(
            payload
                .files
                .iter()
                .map(|file| file.path.to_string_lossy().into_owned())
                .collect(),
        ));
    }
    if let Ok(mut suppression) = suppression.lock() {
        *suppression = Some(SuppressedClipboard {
            fingerprint: payload.fingerprint(),
            applied_at: std::time::Instant::now(),
        });
    }
    context
        .set(contents)
        .map_err(|error| ClipboardAdapterError::WriteFailed(error.to_string()))
}

fn capture_clipboard() -> Result<Option<ClipboardPayload>, ClipboardAdapterError> {
    let context = ClipboardContext::new()
        .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
    let text = context
        .has(ContentFormat::Text)
        .then(|| context.get_text().ok())
        .flatten();
    let html = context
        .has(ContentFormat::Html)
        .then(|| context.get_html().ok())
        .flatten();
    let rich_text = context
        .has(ContentFormat::Rtf)
        .then(|| context.get_rich_text().ok())
        .flatten();
    let image_png = context
        .has(ContentFormat::Image)
        .then(|| {
            context
                .get_image()
                .and_then(|image| image.to_png())
                .map(|image| image.get_bytes().to_vec())
                .ok()
        })
        .flatten();
    let files = context
        .has(ContentFormat::Files)
        .then(|| context.get_files().ok())
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file")
                .to_owned();
            ClipboardFile {
                name,
                path: path.into(),
            }
        })
        .collect();
    let payload = ClipboardPayload {
        id: next_clipboard_id(),
        captured_at_unix_ms: unix_time_ms(),
        text,
        html,
        rich_text,
        image_png,
        files,
    };
    Ok((!payload.is_empty()).then_some(payload))
}

fn next_clipboard_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        unix_time_ms(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardAdapterError {
    #[error("clipboard is unavailable: {0}")]
    Unavailable(String),
    #[error("clipboard content is invalid: {0}")]
    InvalidContent(String),
    #[error("clipboard update failed: {0}")]
    WriteFailed(String),
    #[error("clipboard writer has stopped")]
    WriterStopped,
}
