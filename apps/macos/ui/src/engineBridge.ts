import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  parseAppSnapshot,
  type AppSnapshot,
} from "@kvmflow/ui-contracts";

export async function getInitialSnapshot(): Promise<AppSnapshot> {
  return parseAppSnapshot(await invoke("get_snapshot"));
}

export async function getAutostartEnabled(): Promise<boolean> {
  return invoke<boolean>("get_autostart_enabled");
}

export async function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_autostart_enabled", { enabled });
}

export async function listenForSnapshots(
  onSnapshot: (snapshot: AppSnapshot) => void,
  onMalformedSnapshot?: () => void,
): Promise<UnlistenFn> {
  return listen<unknown>("engine://snapshot", (event) => {
    try {
      onSnapshot(parseAppSnapshot(event.payload));
    } catch {
      onMalformedSnapshot?.();
    }
  });
}

export async function exportFoundationReport(): Promise<string> {
  return invoke<string>("export_foundation_report");
}

export async function configureListenPort(port: number): Promise<void> {
  return invoke("configure_listen_port", { port });
}

export async function confirmPairing(): Promise<void> {
  return invoke("confirm_pairing");
}

export async function rejectPairing(): Promise<void> {
  return invoke("reject_pairing");
}

export async function setEmergencyStopActive(active: boolean): Promise<void> {
  return invoke("set_local_control_allowed", { allowed: !active });
}

export async function refreshAccessibilityPermission(): Promise<void> {
  return invoke("refresh_accessibility_permission");
}

export async function openAccessibilitySettings(): Promise<void> {
  return invoke("open_accessibility_settings");
}

export async function returnControl(): Promise<void> {
  return invoke("return_control");
}

export async function resetInput(): Promise<void> {
  return invoke("reset_input");
}
