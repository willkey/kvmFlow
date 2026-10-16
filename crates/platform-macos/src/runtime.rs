use kvmflow_platform_api::{
    ClipboardAdapter, PlatformBridge, PlatformCommand, PlatformDriver, PlatformEvent,
    platform_bridge,
};

use crate::{
    InjectionOutcome, MacInjectionState, accessibility_permission, open_accessibility_settings,
};

const PLATFORM_CHANNEL_CAPACITY: usize = 256;

#[cfg(target_os = "macos")]
struct MacWakeObserver {
    center: objc2::rc::Retained<objc2_foundation::NSNotificationCenter>,
    token:
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>,
}

#[cfg(target_os = "macos")]
impl MacWakeObserver {
    fn register(events: tokio::sync::mpsc::Sender<PlatformEvent>) -> Self {
        use std::ptr::NonNull;

        use block2::RcBlock;
        use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
        use objc2_foundation::NSNotification;

        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let callback = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            tracing::info!(component = "connection", "Mac system wake detected");
            let _ = events.try_send(PlatformEvent::SystemResumed);
        });
        // SAFETY: The notification name is a valid AppKit constant, no object filter is used,
        // and the copied block owns a thread-safe Tokio sender for the observer lifetime.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                None,
                &callback,
            )
        };
        tracing::info!(
            component = "connection",
            "Mac sleep-wake observer initialized"
        );
        Self { center, token }
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacWakeObserver {
    fn drop(&mut self) {
        let token: &objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol> =
            &self.token;
        let observer: &objc2::runtime::AnyObject = token.as_ref();
        // SAFETY: The token was returned by this notification center and remains alive here.
        unsafe { self.center.removeObserver(observer) };
    }
}

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
    #[cfg(target_os = "macos")]
    let _wake_observer = MacWakeObserver::register(driver.events.clone());
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
            PlatformCommand::ResetInput => {
                let result = injection.release_all();
                if result.is_ok() {
                    injection = MacInjectionState::default();
                    tracing::info!(component = "input", "Mac input state reset completed");
                }
                result
            }
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

        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};

            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            // SAFETY: The test posts the valid AppKit wake notification without an object.
            unsafe {
                center.postNotificationName_object(NSWorkspaceDidWakeNotification, None);
            }
            let resumed = tokio::time::timeout(Duration::from_secs(1), bridge.events.recv())
                .await
                .expect("resume event timeout")
                .expect("resume event");
            assert_eq!(resumed, PlatformEvent::SystemResumed);
        }

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
