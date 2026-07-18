use kvmflow_engine::EngineHandle;
use tauri::{
    AppHandle,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

use crate::window::ensure_main_window;

const OPEN_ID: &str = "open";
const EXIT_ID: &str = "exit";
const OPEN_LABEL: &str = "打开 kvmFlow";
const EXIT_LABEL: &str = "退出";

fn tray_actions() -> &'static [(&'static str, &'static str); 2] {
    &[(OPEN_ID, OPEN_LABEL), (EXIT_ID, EXIT_LABEL)]
}

async fn shutdown_then_exit(engine: &EngineHandle, exit: impl FnOnce()) {
    if let Err(error) = engine.shutdown().await {
        tracing::error!(component = "tray", %error, "failed to persist final validation report");
    }
    exit();
}

pub fn create_tray(app: &AppHandle, engine: &EngineHandle) -> tauri::Result<()> {
    let [(open_id, open_label), (exit_id, exit_label)] = *tray_actions();
    let open = MenuItem::with_id(app, open_id, open_label, true, None::<&str>)?;
    let exit = MenuItem::with_id(app, exit_id, exit_label, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &exit])?;
    let engine = engine.clone();
    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            OPEN_ID => match ensure_main_window(app, &engine) {
                Ok(window) => {
                    if let Err(error) = window.set_focus() {
                        tracing::error!(component = "tray", %error, "failed to focus main window");
                    }
                }
                Err(error) => {
                    tracing::error!(component = "tray", %error, "failed to open main window");
                }
            },
            EXIT_ID => {
                let app = app.clone();
                let engine = engine.clone();
                tauri::async_runtime::spawn(async move {
                    shutdown_then_exit(&engine, || app.exit(0)).await;
                });
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use kvmflow_domain::{RuntimePhase, WindowState};
    use kvmflow_engine::{EngineCommand, EngineError};

    use super::{shutdown_then_exit, tray_actions};
    use crate::test_support::test_engine;

    #[test]
    fn tray_configuration_exposes_exactly_open_and_exit() {
        assert_eq!(
            tray_actions(),
            &[("open", "打开 kvmFlow"), ("exit", "退出")]
        );
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
