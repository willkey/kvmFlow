use std::sync::Arc;

use kvmflow_engine::{EngineCommand, EngineHandle};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const MAIN_WINDOW_LABEL: &str = "main";

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
            .inner_size(980.0, 720.0)
            .min_inner_size(720.0, 560.0)
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
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use kvmflow_domain::WindowState;
    use tokio::{sync::Mutex, time::timeout};

    use super::{close_window_after_open, record_window_opened};
    use crate::test_support::test_engine;

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
