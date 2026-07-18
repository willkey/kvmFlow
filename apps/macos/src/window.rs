use std::sync::Arc;

use kvmflow_engine::{EngineCommand, EngineHandle};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const MAIN_WINDOW_LABEL: &str = "main";

fn log_window_transition(window_state: kvmflow_domain::WindowState) {
    let transition = match window_state {
        kvmflow_domain::WindowState::Open => "windowOpened",
        kvmflow_domain::WindowState::Closed => "windowClosed",
    };
    tracing::info!(
        component = "window",
        transition,
        "window lifecycle transition"
    );
}

async fn record_window_state(
    engine: &EngineHandle,
    command: EngineCommand,
    expected: kvmflow_domain::WindowState,
) -> Result<(), kvmflow_engine::EngineError> {
    let mut snapshots = engine.subscribe();
    drop(snapshots.borrow_and_update());
    engine.send(command).await?;

    loop {
        snapshots
            .changed()
            .await
            .map_err(|_| kvmflow_engine::EngineError::Stopped)?;
        if snapshots.borrow_and_update().window_state == expected {
            log_window_transition(expected);
            return Ok(());
        }
    }
}

async fn record_window_opened(engine: &EngineHandle) -> Result<(), kvmflow_engine::EngineError> {
    record_window_state(
        engine,
        EngineCommand::WindowOpened,
        kvmflow_domain::WindowState::Open,
    )
    .await
}

async fn close_window(engine: &EngineHandle, destroy: impl FnOnce() -> tauri::Result<()>) {
    if let Err(error) = record_window_state(
        engine,
        EngineCommand::WindowClosed,
        kvmflow_domain::WindowState::Closed,
    )
    .await
    {
        tracing::error!(component = "window", %error, "failed to record window close");
    }
    if let Err(error) = destroy() {
        tracing::error!(component = "window", %error, "failed to destroy webview window");
    }
}

async fn close_window_after_open(
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    engine: &EngineHandle,
    destroy: impl FnOnce() -> tauri::Result<()>,
) {
    let _lifecycle_guard = lifecycle.lock_owned().await;
    close_window(engine, destroy).await;
}

pub fn ensure_main_window(app: &AppHandle, engine: &EngineHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        return Ok(window);
    }

    let window =
        WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
            .title("kvmFlow")
            .inner_size(860.0, 640.0)
            .min_inner_size(680.0, 520.0)
            .center()
            .resizable(true)
            .build()?;

    let lifecycle = Arc::new(tokio::sync::Mutex::new(()));
    let open_guard = lifecycle
        .clone()
        .try_lock_owned()
        .expect("new window lifecycle gate must be unlocked");
    let engine_for_open = engine.clone();
    tauri::async_runtime::spawn(async move {
        let _open_guard = open_guard;
        if let Err(error) = record_window_opened(&engine_for_open).await {
            tracing::error!(component = "window", %error, "failed to record window open");
        }
    });

    let window_for_close = window.clone();
    let engine_for_close = engine.clone();
    let lifecycle_for_close = lifecycle;
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let window = window_for_close.clone();
            let engine = engine_for_close.clone();
            let lifecycle = lifecycle_for_close.clone();
            tauri::async_runtime::spawn(async move {
                close_window_after_open(lifecycle, &engine, || window.destroy()).await;
            });
        }
    });

    Ok(window)
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        sync::{
            Arc, Mutex as StdMutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    use kvmflow_domain::WindowState;
    use tokio::{sync::Mutex, time::timeout};
    use tracing_subscriber::fmt::MakeWriter;

    use super::{close_window_after_open, log_window_transition, record_window_opened};
    use crate::test_support::test_engine;

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<StdMutex<Vec<u8>>>);

    impl Write for CapturedLogs {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock captured logs").extend(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn open_and_close_logs_use_ordered_stable_lifecycle_records() {
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            log_window_transition(WindowState::Open);
            log_window_transition(WindowState::Closed);
        });

        let output = String::from_utf8(logs.0.lock().expect("lock captured logs").clone())
            .expect("captured logs are UTF-8");
        let opened = "\"transition\":\"windowOpened\"";
        let closed = "\"transition\":\"windowClosed\"";
        assert_eq!(output.matches(opened).count(), 1);
        assert_eq!(output.matches(closed).count(), 1);
        assert!(output.find(opened) < output.find(closed));
    }

    #[tokio::test]
    async fn opening_records_the_engine_window_state() {
        let (_diagnostics, engine) = test_engine("window-open-test");

        record_window_opened(&engine)
            .await
            .expect("record window open");

        assert_eq!(engine.current_snapshot().window_state, WindowState::Open);
        engine.shutdown().await.expect("shut down engine");
    }

    #[tokio::test]
    async fn rapid_close_waits_for_open_then_destroys_without_hanging() {
        let (_diagnostics, engine) = test_engine("window-close-test");
        let lifecycle = Arc::new(Mutex::new(()));
        let open_guard = lifecycle
            .clone()
            .try_lock_owned()
            .expect("new lifecycle gate is unlocked");
        let destroyed = Arc::new(AtomicBool::new(false));
        let close_task = {
            let lifecycle = lifecycle.clone();
            let engine = engine.clone();
            let destroyed = destroyed.clone();
            tokio::spawn(async move {
                close_window_after_open(lifecycle, &engine, || {
                    assert_eq!(engine.current_snapshot().window_state, WindowState::Closed);
                    destroyed.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await;
            })
        };

        tokio::task::yield_now().await;
        assert!(!destroyed.load(Ordering::SeqCst));
        record_window_opened(&engine)
            .await
            .expect("record window open");
        drop(open_guard);

        timeout(std::time::Duration::from_secs(1), close_task)
            .await
            .expect("serialized close timed out")
            .expect("serialized close task panicked");
        assert!(destroyed.load(Ordering::SeqCst));
        engine.shutdown().await.expect("shut down engine");
    }
}
