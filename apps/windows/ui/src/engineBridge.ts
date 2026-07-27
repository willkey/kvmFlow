import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  parseAppSnapshot,
  parseDisplayTargets,
  type AppSnapshot,
  type DisplayTarget,
  type ScreenEdge,
} from "@kvmflow/ui-contracts";

export async function getInitialSnapshot(): Promise<AppSnapshot> {
  return parseAppSnapshot(await invoke("get_snapshot"));
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

export async function connectManual(ip: string, port: number): Promise<void> {
  return invoke("connect_manual", { ip, port });
}

export async function confirmPairing(): Promise<void> {
  return invoke("confirm_pairing");
}

export async function rejectPairing(): Promise<void> {
  return invoke("reject_pairing");
}

export async function enterControl(
  edge: ScreenEdge,
  position = 0.5,
): Promise<void> {
  return invoke("enter_control", { edge, position });
}

export async function configureScreenEdge(edge: ScreenEdge): Promise<void> {
  return invoke("configure_screen_edge", { edge });
}

export async function listDisplays(): Promise<DisplayTarget[]> {
  return parseDisplayTargets(await invoke("list_displays"));
}

export async function configureTargetDisplay(displayId: string): Promise<void> {
  return invoke("configure_target_display", { displayId });
}

export async function returnControl(): Promise<void> {
  return invoke("return_control");
}

export async function setControlEnabled(enabled: boolean): Promise<void> {
  return invoke("set_control_enabled", { enabled });
}

export async function setGameModeEnabled(enabled: boolean): Promise<void> {
  return invoke("set_game_mode_enabled", { enabled });
}

export async function activateClipboardHistory(id: string): Promise<void> {
  return invoke("activate_clipboard_history", { id });
}

export async function clearClipboardHistory(): Promise<void> {
  return invoke("clear_clipboard_history");
}
