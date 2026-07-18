use kvmflow_platform_api::{PlatformBridge, PlatformCommand, PlatformEvent, platform_bridge};

use crate::{
    InjectionOutcome, MacInjectionState, accessibility_permission, open_accessibility_settings,
};

const PLATFORM_CHANNEL_CAPACITY: usize = 256;

pub fn spawn_macos_platform_adapter() -> PlatformBridge {
    let (mut driver, bridge) = platform_bridge(PLATFORM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let mut injection = MacInjectionState::default();
        let _ = driver
            .events
            .send(PlatformEvent::PermissionChanged(accessibility_permission()))
            .await;

        while let Some(command) = driver.commands.recv().await {
            let result = match command {
                PlatformCommand::Inject(event) => match injection.inject(event) {
                    Ok(InjectionOutcome::Applied) => Ok(()),
                    Ok(InjectionOutcome::ReturnEdgeReached { position }) => {
                        let _ = driver
                            .events
                            .send(PlatformEvent::RemoteEdgeReached { position })
                            .await;
                        Ok(())
                    }
                    Err(error) => Err(error),
                },
                PlatformCommand::ReleaseAll => injection.release_all(),
                PlatformCommand::PrepareRemotePointer { edge, position } => {
                    match injection.prepare_remote_pointer(edge, position) {
                        Ok(()) => {
                            let _ = driver
                                .events
                                .send(PlatformEvent::RemotePointerPrepared)
                                .await;
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                PlatformCommand::RefreshPermission => {
                    let _ = driver
                        .events
                        .send(PlatformEvent::PermissionChanged(accessibility_permission()))
                        .await;
                    Ok(())
                }
                PlatformCommand::OpenAccessibilitySettings => {
                    open_accessibility_settings().map_err(|_| crate::InjectionError::Unavailable)
                }
                PlatformCommand::Shutdown => {
                    let _ = injection.release_all();
                    return;
                }
                PlatformCommand::SetInputSuppressed(_)
                | PlatformCommand::SetEdgeSwitchEnabled(_)
                | PlatformCommand::SetEdgeSwitchEdge(_)
                | PlatformCommand::BeginEdgeTransition { .. }
                | PlatformCommand::RestoreLocalPointer { .. } => Ok(()),
            };
            if let Err(error) = result {
                let _ = driver
                    .events
                    .send(PlatformEvent::AdapterFailed(error.to_string()))
                    .await;
            }
        }
        let _ = injection.release_all();
    });
    bridge
}
