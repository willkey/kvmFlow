use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clipboard_rs::{
    Clipboard, ClipboardContent, ClipboardContext, ClipboardHandler, ClipboardWatcher,
    ClipboardWatcherContext, ContentFormat, WatcherShutdown, common::RustImage,
};
use kvmflow_domain::{ClipboardFile, ClipboardPayload};
use tokio::sync::mpsc::Sender;

use crate::PlatformEvent;

// Applications replace these formats on a new copy. Unlike a time window, this
// marker suppresses our own echo without dropping an immediately following copy.
const SOURCE_FORMAT: &str = "com.kvmflow.clipboard-source";
const SOURCE_MARKER: &[u8] = b"kvmflow-v1";
const CLIPBOARD_ATTEMPTS: usize = 6;
const CLIPBOARD_RETRY_DELAY: Duration = Duration::from_millis(20);

#[cfg(target_os = "windows")]
mod windows;

pub struct ClipboardAdapter {
    writer: ClipboardWriterHandle,
    watcher: ClipboardWatcherHandle,
}

impl ClipboardAdapter {
    pub fn start(events: Sender<PlatformEvent>) -> Result<Self, ClipboardAdapterError> {
        let access = Arc::new(Mutex::new(()));
        let mut watcher = ClipboardWatcherContext::new_with_interval(Duration::from_millis(500))
            .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
        let initial = capture_clipboard(&access);
        let handler = ChangeHandler {
            events: events.clone(),
            access: access.clone(),
        };
        watcher.add_handler(handler);
        let (writer_tx, writer_rx) = mpsc::channel();
        let writer_events = events.clone();
        let writer_join = std::thread::Builder::new()
            .name("kvmflow-clipboard-writer".into())
            .spawn(move || {
                while let Ok(payload) = writer_rx.recv() {
                    let started = Instant::now();
                    if let Err(error) = apply_clipboard(&access, &payload) {
                        tracing::warn!(component = "clipboard", transfer_id = %payload.id,
                            elapsed_ms = started.elapsed().as_millis() as u64, %error,
                            "native clipboard apply failed");
                        let _ = writer_events
                            .try_send(PlatformEvent::ClipboardFailed(error.to_string()));
                    } else {
                        tracing::info!(component = "clipboard", transfer_id = %payload.id,
                            kind = ?payload.kind(), file_count = payload.files.len(),
                            inline_bytes = payload.total_inline_bytes(),
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            "native clipboard apply completed");
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
    access: Arc<Mutex<()>>,
}

impl ClipboardHandler for ChangeHandler {
    fn on_clipboard_change(&mut self) {
        match capture_clipboard(&self.access) {
            Ok(Some(payload)) => {
                let _ = self
                    .events
                    .blocking_send(PlatformEvent::ClipboardChanged(payload));
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

fn apply_clipboard(
    access: &Mutex<()>,
    payload: &ClipboardPayload,
) -> Result<(), ClipboardAdapterError> {
    let contents = clipboard_contents(payload)?;
    let _guard = access.lock().unwrap_or_else(|error| error.into_inner());
    #[cfg(target_os = "windows")]
    return windows::write(contents);
    #[cfg(not(target_os = "windows"))]
    {
        let context = ClipboardContext::new()
            .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
        context
            .set(contents)
            .map_err(|error| ClipboardAdapterError::WriteFailed(error.to_string()))?;
        if context.get_buffer(SOURCE_FORMAT).ok().as_deref() != Some(SOURCE_MARKER) {
            return Err(ClipboardAdapterError::WriteFailed(
                "source marker was not published".into(),
            ));
        }
        // clipboard-rs may ignore an individual representation's write failure.
        for (required, format, name) in [
            (payload.text.is_some(), ContentFormat::Text, "text"),
            (payload.html.is_some(), ContentFormat::Html, "html"),
            (payload.rich_text.is_some(), ContentFormat::Rtf, "rtf"),
            (payload.image_png.is_some(), ContentFormat::Image, "image"),
            (!payload.files.is_empty(), ContentFormat::Files, "files"),
        ] {
            if required && !context.has(format.clone()) {
                return Err(ClipboardAdapterError::WriteFailed(format!(
                    "missing native format: {name}"
                )));
            }
        }
        if !payload.files.is_empty() {
            let files = context
                .get_files()
                .map_err(|error| ClipboardAdapterError::WriteFailed(error.to_string()))?;
            if files.len() != payload.files.len() {
                return Err(ClipboardAdapterError::WriteFailed(
                    "not all file references were published".into(),
                ));
            }
        }
        Ok(())
    }
}

fn clipboard_contents(
    payload: &ClipboardPayload,
) -> Result<Vec<ClipboardContent>, ClipboardAdapterError> {
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
    contents.push(ClipboardContent::Other(
        SOURCE_FORMAT.into(),
        SOURCE_MARKER.to_vec(),
    ));
    Ok(contents)
}

fn capture_clipboard(
    access: &Mutex<()>,
) -> Result<Option<ClipboardPayload>, ClipboardAdapterError> {
    let _guard = access.lock().unwrap_or_else(|error| error.into_inner());
    retry_clipboard_operation("capture", capture_clipboard_once, std::thread::sleep)
}

fn retry_clipboard_operation<T>(
    operation: &'static str,
    mut action: impl FnMut() -> Result<T, ClipboardAdapterError>,
    mut wait: impl FnMut(Duration),
) -> Result<T, ClipboardAdapterError> {
    for attempt in 1..=CLIPBOARD_ATTEMPTS {
        match action() {
            Ok(value) => {
                if attempt > 1 {
                    tracing::info!(
                        component = "clipboard",
                        operation,
                        attempts = attempt,
                        "native clipboard access recovered"
                    );
                }
                return Ok(value);
            }
            Err(error)
                if attempt < CLIPBOARD_ATTEMPTS
                    && matches!(
                        error,
                        ClipboardAdapterError::ReadFailed(_)
                            | ClipboardAdapterError::Unavailable(_)
                    ) =>
            {
                wait(CLIPBOARD_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("clipboard attempts are nonzero")
}

fn read_optional_content<T>(
    present: bool,
    read: impl FnOnce() -> clipboard_rs::common::Result<T>,
) -> Result<Option<T>, ClipboardAdapterError> {
    present
        .then(read)
        .transpose()
        .map_err(|error| ClipboardAdapterError::ReadFailed(error.to_string()))
}

fn capture_clipboard_once() -> Result<Option<ClipboardPayload>, ClipboardAdapterError> {
    let context = ClipboardContext::new()
        .map_err(|error| ClipboardAdapterError::Unavailable(error.to_string()))?;
    let marker = read_optional_content(
        context.has(ContentFormat::Other(SOURCE_FORMAT.into())),
        || context.get_buffer(SOURCE_FORMAT),
    )?;
    capture_unmarked(marker.as_deref(), || capture_payload(&context))
}

fn capture_unmarked(
    marker: Option<&[u8]>,
    capture: impl FnOnce() -> Result<Option<ClipboardPayload>, ClipboardAdapterError>,
) -> Result<Option<ClipboardPayload>, ClipboardAdapterError> {
    if marker == Some(SOURCE_MARKER) {
        Ok(None)
    } else {
        capture()
    }
}

fn capture_payload(
    context: &ClipboardContext,
) -> Result<Option<ClipboardPayload>, ClipboardAdapterError> {
    let text = read_optional_content(context.has(ContentFormat::Text), || context.get_text())?;
    let html = read_optional_content(context.has(ContentFormat::Html), || context.get_html())?;
    let rich_text =
        read_optional_content(context.has(ContentFormat::Rtf), || context.get_rich_text())?;
    let image_png = read_optional_content(context.has(ContentFormat::Image), || {
        context
            .get_image()
            .and_then(|image| image.to_png())
            .map(|image| image.get_bytes().to_vec())
    })?;
    let files = read_optional_content(context.has(ContentFormat::Files), || context.get_files())?
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
    #[error("clipboard read failed: {0}")]
    ReadFailed(String),
    #[error("clipboard writer has stopped")]
    WriterStopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> ClipboardPayload {
        ClipboardPayload {
            id: "test-copy".into(),
            captured_at_unix_ms: 1,
            text: Some("copy".into()),
            html: None,
            rich_text: None,
            image_png: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn remote_marker_suppresses_echo_but_not_an_immediate_new_copy() {
        assert!(
            capture_unmarked(Some(SOURCE_MARKER), || panic!(
                "own write must not be recaptured"
            ))
            .unwrap()
            .is_none()
        );
        assert_eq!(
            capture_unmarked(None, || Ok(Some(payload()))).unwrap(),
            Some(payload())
        );
        assert_eq!(
            capture_unmarked(Some(b"unrecognized"), || Ok(Some(payload()))).unwrap(),
            Some(payload())
        );
        let contents = clipboard_contents(&payload()).unwrap();
        assert!(matches!(contents.first(), Some(ClipboardContent::Text(text)) if text == "copy"));
        assert!(
            matches!(contents.last(), Some(ClipboardContent::Other(format, marker)) if format == SOURCE_FORMAT && marker == SOURCE_MARKER)
        );
    }

    #[test]
    fn advertised_read_errors_are_not_treated_as_an_empty_clipboard() {
        let result = read_optional_content::<String>(true, || Err("clipboard busy".into()));
        assert!(matches!(result, Err(ClipboardAdapterError::ReadFailed(_))));
        assert_eq!(
            read_optional_content::<String>(false, || panic!("absent format must not be read"))
                .unwrap(),
            None
        );
    }

    #[test]
    fn busy_clipboard_is_retried_without_another_copy_event() {
        let mut attempts = 0;
        let mut delays = Vec::new();
        let captured = retry_clipboard_operation(
            "test",
            || {
                attempts += 1;
                read_optional_content(true, || {
                    if attempts == 1 {
                        Err("busy".into())
                    } else {
                        Ok("ready")
                    }
                })
            },
            |delay| delays.push(delay),
        )
        .unwrap();
        assert_eq!(captured, Some("ready"));
        assert_eq!(attempts, 2);
        assert_eq!(delays, [CLIPBOARD_RETRY_DELAY]);
    }

    #[test]
    fn retries_are_bounded_and_never_replay_a_failed_write() {
        let mut attempts = 0;
        let result = retry_clipboard_operation::<()>(
            "test",
            || {
                attempts += 1;
                Err(ClipboardAdapterError::Unavailable("busy".into()))
            },
            |_| {},
        );
        assert!(result.is_err());
        assert_eq!(attempts, CLIPBOARD_ATTEMPTS);
        let result = retry_clipboard_operation::<()>(
            "test",
            || Err(ClipboardAdapterError::WriteFailed("partial write".into())),
            |_| panic!("must not replay writes"),
        );
        assert!(result.is_err());
    }
}
