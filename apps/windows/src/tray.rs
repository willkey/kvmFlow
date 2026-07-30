use kvmflow_domain::{ClipboardContentKind, ClipboardHistoryItem};
use kvmflow_engine::{EngineCommand, EngineHandle};
use tauri::{
    AppHandle,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
};

use crate::window::ensure_main_window;

const HISTORY_ID: &str = "clipboard-history";
const HISTORY_ITEM_PREFIX: &str = "clipboard-history:";
const OPEN_ID: &str = "open";
const EXIT_ID: &str = "exit";
const OPEN_LABEL: &str = "打开 kvmFlow";
const EXIT_LABEL: &str = "退出";

fn tray_actions() -> &'static [(&'static str, &'static str); 2] {
    &[(OPEN_ID, OPEN_LABEL), (EXIT_ID, EXIT_LABEL)]
}

fn history_item_id(id: &str) -> String {
    format!("{HISTORY_ITEM_PREFIX}{id}")
}

fn history_id_from_action(action: &str) -> Option<&str> {
    action.strip_prefix(HISTORY_ITEM_PREFIX)
}

fn history_label(item: &ClipboardHistoryItem) -> String {
    let kind = match item.kind {
        ClipboardContentKind::Text => "文本",
        ClipboardContentKind::Html => "HTML",
        ClipboardContentKind::RichText => "富文本",
        ClipboardContentKind::Image => "图片",
        ClipboardContentKind::Files => "文件",
        ClipboardContentKind::Mixed => "混合",
    };
    let preview = item
        .preview
        .chars()
        .take(48)
        .collect::<String>()
        .replace('&', "&&");
    if preview.is_empty() {
        kind.into()
    } else {
        format!("{kind} · {preview}")
    }
}

fn refresh_history_menu(
    app: &AppHandle,
    menu: &Submenu<tauri::Wry>,
    history: &[ClipboardHistoryItem],
) -> tauri::Result<()> {
    while !menu.items()?.is_empty() {
        menu.remove_at(0)?;
    }
    if history.is_empty() {
        menu.append(&MenuItem::new(app, "暂无剪贴板历史", false, None::<&str>)?)?;
        return Ok(());
    }
    for item in history {
        menu.append(&MenuItem::with_id(
            app,
            history_item_id(&item.id),
            history_label(item),
            item.available,
            None::<&str>,
        )?)?;
    }
    Ok(())
}

fn start_history_menu_updates(app: &AppHandle, menu: Submenu<tauri::Wry>, engine: &EngineHandle) {
    let app = app.clone();
    let mut snapshots = engine.subscribe();
    tauri::async_runtime::spawn(async move {
        let mut previous = snapshots.borrow().kvm.clipboard.history.clone();
        while snapshots.changed().await.is_ok() {
            let history = snapshots.borrow_and_update().kvm.clipboard.history.clone();
            if history == previous {
                continue;
            }
            previous.clone_from(&history);
            if let Err(error) = refresh_history_menu(&app, &menu, &history) {
                tracing::error!(
                    component = "tray",
                    %error,
                    "failed to refresh clipboard history menu"
                );
            }
        }
    });
}

async fn shutdown_then_exit(engine: &EngineHandle, exit: impl FnOnce()) {
    if let Err(error) = engine.shutdown().await {
        tracing::error!(component = "tray", %error, "failed to persist final validation report");
    }
    exit();
}

pub fn create_tray(app: &AppHandle, engine: &EngineHandle) -> tauri::Result<()> {
    let [(open_id, open_label), (exit_id, exit_label)] = *tray_actions();
    let history_menu = Submenu::with_id(app, HISTORY_ID, "剪贴板历史", true)?;
    refresh_history_menu(
        app,
        &history_menu,
        &engine.current_snapshot().kvm.clipboard.history,
    )?;
    let open = MenuItem::with_id(app, open_id, open_label, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let exit = MenuItem::with_id(app, exit_id, exit_label, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&history_menu, &open, &separator, &exit])?;
    let engine_for_menu = engine.clone();
    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let action = event.id().as_ref();
            if let Some(id) = history_id_from_action(action) {
                let engine = engine_for_menu.clone();
                let id = id.to_owned();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = engine
                        .send(EngineCommand::ActivateClipboardHistory(id))
                        .await
                    {
                        tracing::error!(
                            component = "tray",
                            %error,
                            "failed to activate clipboard history"
                        );
                    }
                });
                return;
            }
            match action {
                OPEN_ID => match ensure_main_window(app, &engine_for_menu) {
                    Ok(window) => {
                        if let Err(error) = window.set_focus() {
                            tracing::error!(
                                component = "tray",
                                %error,
                                "failed to focus main window"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::error!(
                            component = "tray",
                            %error,
                            "failed to open main window"
                        );
                    }
                },
                EXIT_ID => {
                    let app = app.clone();
                    let engine = engine_for_menu.clone();
                    tauri::async_runtime::spawn(async move {
                        shutdown_then_exit(&engine, || app.exit(0)).await;
                    });
                }
                _ => {}
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    start_history_menu_updates(app, history_menu, engine);

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use kvmflow_domain::{ClipboardContentKind, ClipboardHistoryItem, RuntimePhase, WindowState};
    use kvmflow_engine::{EngineCommand, EngineError};

    use super::{
        history_id_from_action, history_item_id, history_label, shutdown_then_exit, tray_actions,
    };
    use crate::test_support::test_engine;

    #[test]
    fn tray_configuration_exposes_exactly_open_and_exit() {
        assert_eq!(
            tray_actions(),
            &[("open", "打开 kvmFlow"), ("exit", "退出")]
        );
    }

    #[test]
    fn clipboard_history_actions_round_trip_and_labels_are_bounded() {
        let id = "history-42";
        assert_eq!(history_id_from_action(&history_item_id(id)), Some(id));
        let item = ClipboardHistoryItem {
            id: id.into(),
            captured_at_unix_ms: 42,
            kind: ClipboardContentKind::Text,
            preview: format!("A&B {}", "x".repeat(80)),
            size_bytes: 84,
            available: true,
        };
        let label = history_label(&item);
        assert!(label.starts_with("文本 · A&&B"));
        assert!(label.chars().count() <= 55);
    }

    #[tokio::test]
    async fn shutdown_completes_before_the_exit_callback() {
        let (diagnostics, engine) = test_engine("tray-exit-test");
        let mut exited = false;

        shutdown_then_exit(&engine, || {
            let snapshot = engine.current_snapshot();
            assert_eq!(snapshot.phase, RuntimePhase::Stopping);
            assert_eq!(snapshot.window_state, WindowState::Closed);
            assert_eq!(
                fs::read_dir(diagnostics.path())
                    .expect("read diagnostics directory")
                    .count(),
                1
            );
            exited = true;
        })
        .await;

        assert!(exited);
        assert!(matches!(
            engine.send(EngineCommand::WindowOpened).await,
            Err(EngineError::Stopped)
        ));
    }
}
