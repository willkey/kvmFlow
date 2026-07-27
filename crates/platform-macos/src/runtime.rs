use kvmflow_platform_api::{
    ClipboardAdapter, PlatformBridge, PlatformCommand, PlatformDriver, PlatformEvent,
    platform_bridge,
};

use crate::{
    InjectionOutcome, MacInjectionState, accessibility_permission, open_accessibility_settings,
};

const PLATFORM_CHANNEL_CAPACITY: usize = 256;

pub fn spawn_macos_platform_adapter() -> PlatformBridge {
    let (driver, bridge) = platform_bridge(PLATFORM_CHANNEL_CAPACITY);
    let failure_events = driver.events.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("kvmflow-macos-input".to_owned())
        .spawn(move || run_macos_platform_adapter(driver))
    {
        let _ = failure_events.try_send(PlatformEvent::AdapterFailed(format!(
            "failed to start macOS input adapter: {error}"
        )));
    }
    bridge
}

fn run_macos_platform_adapter(mut driver: PlatformDriver) {
    let mut injection = MacInjectionState::default();
    let _ = driver
        .events
        .blocking_send(PlatformEvent::PermissionChanged(accessibility_permission()));
    let clipboard = ClipboardAdapter::start(driver.events.clone());
    if let Err(error) = clipboard.as_ref() {
        let _ = driver
            .events
            .blocking_send(PlatformEvent::ClipboardFailed(error.to_string()));
    }

    while let Some(command) = driver.commands.blocking_recv() {
        let result = match command {
            PlatformCommand::Inject(event) => {
                match with_platform_autorelease_pool(|| injection.inject(event)) {
                    Ok(InjectionOutcome::Applied) => Ok(()),
                    Ok(InjectionOutcome::ReturnEdgeReached { position }) => {
                        let _ = driver
                            .events
                            .blocking_send(PlatformEvent::RemoteEdgeReached { position });
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            PlatformCommand::ReleaseAll => injection.release_all(),
            PlatformCommand::PrepareRemotePointer { edge, position } => {
                match injection.prepare_remote_pointer(edge, position) {
                    Ok(()) => {
                        let _ = driver
                            .events
                            .blocking_send(PlatformEvent::RemotePointerPrepared);
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            PlatformCommand::RefreshPermission => {
                let _ = driver
                    .events
                    .blocking_send(PlatformEvent::PermissionChanged(accessibility_permission()));
                Ok(())
            }
            PlatformCommand::OpenAccessibilitySettings => {
                open_accessibility_settings().map_err(|_| crate::InjectionError::Unavailable)
            }
            PlatformCommand::ApplyClipboard(payload) => match clipboard.as_ref() {
                Ok(clipboard) => clipboard
                    .apply(payload)
                    .map_err(|_| crate::InjectionError::Unavailable),
                Err(_) => Err(crate::InjectionError::Unavailable),
            },
            PlatformCommand::Shutdown => {
                let _ = injection.release_all();
                return;
            }
            PlatformCommand::SetInputSuppressed(_)
            | PlatformCommand::SetEdgeSwitchEnabled(_)
            | PlatformCommand::SetEdgeSwitchEdge(_)
            | PlatformCommand::SetEdgeSwitchDisplay(_)
            | PlatformCommand::BeginEdgeTransition { .. }
            | PlatformCommand::RestoreLocalPointer { .. } => Ok(()),
        };
        if let Err(error) = result {
            let _ = driver
                .events
                .blocking_send(PlatformEvent::AdapterFailed(error.to_string()));
        }
    }
    let _ = injection.release_all();
}

#[cfg(target_os = "macos")]
fn with_platform_autorelease_pool<T>(work: impl FnOnce() -> T) -> T {
    objc2::rc::autoreleasepool(|_| work())
}

#[cfg(not(target_os = "macos"))]
fn with_platform_autorelease_pool<T>(work: impl FnOnce() -> T) -> T {
    work()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn dedicated_adapter_thread_reports_permission_and_shuts_down() {
        let mut bridge = spawn_macos_platform_adapter();

        let permission = tokio::time::timeout(Duration::from_secs(1), bridge.events.recv())
            .await
            .expect("permission event timeout")
            .expect("permission event");
        assert!(matches!(permission, PlatformEvent::PermissionChanged(_)));

        bridge
            .handle
            .send(PlatformCommand::Shutdown)
            .await
            .expect("send shutdown");
        tokio::time::timeout(Duration::from_secs(2), async {
            while bridge.events.recv().await.is_some() {}
        })
        .await
        .expect("adapter shutdown timeout");
    }

    #[test]
    fn platform_autorelease_pool_returns_the_work_result() {
        assert_eq!(with_platform_autorelease_pool(|| 42), 42);
    }
}
