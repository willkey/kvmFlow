import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshot } from "@kvmflow/ui-contracts";
import App from "./App";

const tauriMocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauriMocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauriMocks.listen }));

type TauriEventHandler = (event: { payload: unknown }) => void;

const initialSnapshot: AppSnapshot = {
  schemaVersion: 6,
  role: "windowsController",
  phase: "running",
  windowState: "open",
  build: { version: "0.1.0", buildId: "fixture-build" },
  diagnosticNotice: null,
  latestResource: null,
  validation: {
    backgroundMemory: {
      status: "notObserved",
      current: null,
      target: 52_428_800,
      unit: "bytes",
    },
    visibleMemory: {
      status: "notObserved",
      current: null,
      target: 188_743_680,
      unit: "bytes",
    },
    backgroundCpu: {
      status: "notObserved",
      current: null,
      target: 0.5,
      unit: "percent",
    },
    continuousUptimeSeconds: 0,
    sampleCount: 0,
    validCpuSampleCount: 0,
    peakMemoryBytes: null,
    averageCpuPercent: null,
    peakCpuPercent: null,
    kvm: {
      successfulEntries: 0,
      successfulReturns: 0,
      transitionFailures: 0,
      emergencyReturns: 0,
      emergencyReturnSampleCount: 0,
      peakEmergencyReturnMs: null,
      disconnects: 0,
      reconnects: 0,
      networkRecoveries: 0,
      wakeRecoveries: 0,
      latencySampleCount: 0,
      averageLatencyMs: null,
      p95LatencyMs: null,
      peakLatencyMs: null,
    },
  },
  kvm: {
    trust: "unpaired",
    link: "idle",
    control: "local",
    permission: "unknown",
    localPolicy: "allowed",
    controlEnabled: true,
    gameModeEnabled: true,
    foregroundFullscreen: false,
    remoteControlAvailable: true,
    clipboard: {
      transferState: "idle",
      lastError: null,
      lastUpdatedUnixMs: null,
      history: [],
    },
    screenEdge: "bottom",
    targetDisplayId: null,
    endpoint: null,
    peerDeviceName: null,
    pairing: null,
    latencyMs: null,
    sessionId: null,
    consecutiveFailures: 0,
    lastError: null,
  },
};

const availableDisplays = [
  {
    id: "\\\\.\\DISPLAY1",
    primary: true,
    left: 0,
    top: 0,
    width: 1920,
    height: 1080,
  },
  {
    id: "\\\\.\\DISPLAY2",
    primary: false,
    left: 1920,
    top: 0,
    width: 2560,
    height: 1440,
  },
];

function cloneSnapshot(): AppSnapshot {
  return structuredClone(initialSnapshot);
}

async function flushPromises(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

function openDiagnostics(): void {
  fireEvent.click(screen.getByText("运行与诊断"));
}

describe("App", () => {
  let eventHandlers: Map<string, TauriEventHandler>;
  let snapshotUnlisten: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    eventHandlers = new Map();
    snapshotUnlisten = vi.fn();
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }

      if (command === "list_displays") {
        return Promise.resolve(availableDisplays);
      }

      if (command === "export_foundation_report") {
        return Promise.resolve("C:\\kvmFlow\\diagnostics\\foundation-report.json");
      }

      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    tauriMocks.listen.mockImplementation(
      async (eventName: string, callback: TauriEventHandler) => {
        eventHandlers.set(eventName, callback);
        return snapshotUnlisten;
      },
    );
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("invokes get_snapshot exactly once on mount", async () => {
    render(<App />);

    await flushPromises();

    expect(tauriMocks.invoke).toHaveBeenCalledTimes(2);
    expect(tauriMocks.invoke).toHaveBeenCalledWith("get_snapshot");
    expect(tauriMocks.invoke).toHaveBeenCalledWith("list_displays");
  });

  it("registers the snapshot listener and unlistens on unmount", async () => {
    const { unmount } = render(<App />);

    await flushPromises();

    expect(tauriMocks.listen).toHaveBeenCalledTimes(1);
    expect(tauriMocks.listen).toHaveBeenCalledWith(
      "engine://snapshot",
      expect.any(Function),
    );
    unmount();

    expect(snapshotUnlisten).toHaveBeenCalledTimes(1);
  });

  it("does not poll after one fake minute", async () => {
    render(<App />);

    await flushPromises();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(tauriMocks.invoke).toHaveBeenCalledTimes(2);
  });

  it("refreshes display topology when the native adapter publishes a system change", async () => {
    const onlineSnapshot = cloneSnapshot();
    onlineSnapshot.kvm.link = "online";
    onlineSnapshot.kvm.trust = "paired";
    onlineSnapshot.kvm.targetDisplayId = "\\\\.\\DISPLAY1";
    let currentDisplays = availableDisplays;
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(onlineSnapshot);
      }
      if (command === "list_displays") {
        return Promise.resolve(currentDisplays);
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await flushPromises();

    expect(screen.getByRole("radio", { name: "右侧" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "下方" })).toBeEnabled();

    currentDisplays = [
      availableDisplays[0],
      {
        ...availableDisplays[1],
        left: 0,
        top: 1080,
      },
    ];
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });
    await flushPromises();

    expect(screen.getByRole("radio", { name: "右侧" })).toBeEnabled();
    expect(screen.getByRole("radio", { name: "下方" })).toBeDisabled();
  });

  it("keeps an event snapshot when the pending initial snapshot resolves later", async () => {
    let resolveInitialSnapshot: ((snapshot: AppSnapshot) => void) | undefined;
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return new Promise<AppSnapshot>((resolve) => {
          resolveInitialSnapshot = resolve;
        });
      }
      if (command === "list_displays") {
        return Promise.resolve(availableDisplays);
      }

      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await flushPromises();

    const eventSnapshot = cloneSnapshot();
    eventSnapshot.build.buildId = "event-build";
    eventSnapshot.latestResource = {
      observedAtUnixMs: 1_700_000_000_000,
      uptimeSeconds: 42,
      windowState: "open",
      processCount: 4,
      residentMemoryBytes: 67_108_864,
      cpuPercent: 0.32,
      cpuValid: true,
    };

    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: eventSnapshot });
    });

    const initialSnapshotAfterEvent = cloneSnapshot();
    initialSnapshotAfterEvent.build.buildId = "stale-initial-build";
    resolveInitialSnapshot?.(initialSnapshotAfterEvent);
    await flushPromises();
    openDiagnostics();

    expect(screen.getByText("event-build")).toBeVisible();
    expect(screen.queryByText("stale-initial-build")).not.toBeInTheDocument();
  });

  it("renders metrics, build ID, and gate status from an emitted snapshot", async () => {
    render(<App />);
    await flushPromises();

    const emittedSnapshot = cloneSnapshot();
    emittedSnapshot.build.buildId = "build-emitted";
    emittedSnapshot.latestResource = {
      observedAtUnixMs: 1_700_000_000_000,
      uptimeSeconds: 42,
      windowState: "open",
      processCount: 4,
      residentMemoryBytes: 67_108_864,
      cpuPercent: 0.32,
      cpuValid: true,
    };
    emittedSnapshot.validation.backgroundMemory.status = "passed";
    emittedSnapshot.validation.visibleMemory.status = "collecting";
    emittedSnapshot.validation.backgroundCpu.status = "failed";
    emittedSnapshot.validation.sampleCount = 12;
    emittedSnapshot.validation.validCpuSampleCount = 11;
    emittedSnapshot.validation.averageCpuPercent = 0.18;
    emittedSnapshot.validation.peakCpuPercent = 0.9;
    emittedSnapshot.validation.peakMemoryBytes = 83_886_080;

    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: emittedSnapshot });
    });
    openDiagnostics();

    expect(screen.getByText("build-emitted")).toBeVisible();
    expect(screen.getByText("4", { selector: ".metric dd" })).toBeVisible();
    expect(screen.getByText("0.32%")).toBeVisible();
    expect(screen.getByText("64.0 MB")).toBeVisible();
    expect(screen.getByText("0.18%")).toBeVisible();
    expect(screen.getByText("0.90%")).toBeVisible();
    expect(screen.getByText("80.0 MB")).toBeVisible();
    expect(screen.getByText(/采样 12 次 · CPU 有效11 次/)).toBeVisible();
    expect(screen.getByText("已通过")).toBeVisible();
    expect(screen.getByText("采集中")).toBeVisible();
    expect(screen.getByText("未通过")).toBeVisible();
  });

  it("shows a persistent notice after three consecutive connection failures", async () => {
    render(<App />);
    await flushPromises();

    const failedSnapshot = cloneSnapshot();
    failedSnapshot.kvm.link = "backoff";
    failedSnapshot.kvm.consecutiveFailures = 3;
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: failedSnapshot });
    });

    expect(screen.getByRole("alert")).toHaveTextContent(
      "已连续重连失败 3 次，程序仍会每 5 秒重试",
    );
  });

  it("renders automatic KVM cycle, recovery, and latency progress", async () => {
    render(<App />);
    await flushPromises();

    const progressSnapshot = cloneSnapshot();
    progressSnapshot.validation.kvm = {
      successfulEntries: 12,
      successfulReturns: 11,
      transitionFailures: 1,
      emergencyReturns: 2,
      emergencyReturnSampleCount: 2,
      peakEmergencyReturnMs: 0.8,
      disconnects: 3,
      reconnects: 3,
      networkRecoveries: 2,
      wakeRecoveries: 1,
      latencySampleCount: 120,
      averageLatencyMs: 4.25,
      p95LatencyMs: 9,
      peakLatencyMs: 15.5,
    };
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: progressSnapshot });
    });
    openDiagnostics();

    expect(screen.getByText("完整切换 11 / 100")).toBeVisible();
    expect(screen.getByText("12 / 11")).toBeVisible();
    expect(screen.getByText("9.0 ms")).toBeVisible();
    expect(screen.getByText("4.3 ms / 15.5 ms")).toBeVisible();
  });

  it("exports the foundation report once and renders its path", async () => {
    render(<App />);
    await flushPromises();

    fireEvent.click(screen.getByRole("button", { name: "导出诊断" }));
    await flushPromises();

    expect(tauriMocks.invoke).toHaveBeenCalledTimes(3);
    expect(tauriMocks.invoke).toHaveBeenLastCalledWith("export_foundation_report");
    expect(
      screen.getByText("C:\\kvmFlow\\diagnostics\\foundation-report.json"),
    ).toBeVisible();
  });

  it("starts control with the expected payload and shows the pending state", async () => {
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }
      if (command === "list_displays") {
        return Promise.resolve(availableDisplays);
      }
      if (command === "enter_control") {
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await flushPromises();

    const onlineSnapshot = cloneSnapshot();
    onlineSnapshot.kvm.link = "online";
    onlineSnapshot.kvm.trust = "paired";
    onlineSnapshot.kvm.endpoint = { ip: "192.168.50.10", port: 45_888 };
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });

    fireEvent.click(screen.getByRole("button", { name: "开始控制" }));
    await flushPromises();
    expect(tauriMocks.invoke).toHaveBeenLastCalledWith("enter_control", {
      edge: "bottom",
      position: 0.5,
    });

    onlineSnapshot.kvm.control = "enteringRemote";
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });
    expect(
      screen.getByRole("button", { name: "正在开始控制" }),
    ).toBeDisabled();
  });

  it("persists the selected Mac position and removes the remote return command", async () => {
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }
      if (command === "list_displays") {
        return Promise.resolve(availableDisplays);
      }
      if (command === "configure_screen_edge") {
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await flushPromises();

    const onlineSnapshot = cloneSnapshot();
    onlineSnapshot.kvm.link = "online";
    onlineSnapshot.kvm.trust = "paired";
    onlineSnapshot.kvm.endpoint = { ip: "192.168.50.10", port: 45_888 };
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });

    fireEvent.click(screen.getByRole("radio", { name: "上方" }));
    await flushPromises();
    expect(tauriMocks.invoke).toHaveBeenLastCalledWith("configure_screen_edge", {
      edge: "top",
    });

    onlineSnapshot.kvm.control = "remote";
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });
    expect(screen.queryByRole("button", { name: "返回本机" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "开始控制" })).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "下方" })).toBeDisabled();
  });

  it("selects which Windows display enters the Mac", async () => {
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }
      if (command === "list_displays") {
        return Promise.resolve(availableDisplays);
      }
      if (command === "configure_target_display") {
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await flushPromises();

    const onlineSnapshot = cloneSnapshot();
    onlineSnapshot.kvm.link = "online";
    onlineSnapshot.kvm.trust = "paired";
    onlineSnapshot.kvm.targetDisplayId = "\\\\.\\DISPLAY1";
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });

    expect(screen.getByRole("radio", { name: "右侧" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "左侧" })).toBeEnabled();
    expect(screen.getByText("灰色方向已有 Windows 显示器，不能用于进入 Mac")).toBeVisible();

    fireEvent.change(
      screen.getByRole("combobox", { name: "进入 Mac 的 Windows 显示器" }),
      { target: { value: "\\\\.\\DISPLAY2" } },
    );
    await flushPromises();

    expect(tauriMocks.invoke).toHaveBeenLastCalledWith(
      "configure_target_display",
      { displayId: "\\\\.\\DISPLAY2" },
    );

    onlineSnapshot.kvm.targetDisplayId = "\\\\.\\DISPLAY2";
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });
    expect(screen.getByRole("radio", { name: "左侧" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "右侧" })).toBeEnabled();
  });

  it("reapplies and clears persistent clipboard history from Windows", async () => {
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        const snapshot = cloneSnapshot();
        snapshot.kvm.clipboard.history = [
          {
            id: "history-text",
            capturedAtUnixMs: 1_700_000_000_000,
            kind: "text",
            preview: "待重新复制的文本",
            sizeBytes: 18,
            available: true,
          },
          {
            id: "history-file",
            capturedAtUnixMs: 1_700_000_100_000,
            kind: "files",
            preview: "已删除的文件.txt",
            sizeBytes: 0,
            available: false,
          },
        ];
        return Promise.resolve(snapshot);
      }
      if (command === "list_displays") {
        return Promise.resolve(availableDisplays);
      }
      if (
        command === "activate_clipboard_history" ||
        command === "clear_clipboard_history"
      ) {
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await flushPromises();

    fireEvent.click(screen.getByText("剪贴板历史"));
    expect(screen.getByText("待重新复制的文本")).toBeVisible();
    expect(screen.getByRole("button", { name: "文件不可用" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "重新复制" }));
    await flushPromises();
    expect(tauriMocks.invoke).toHaveBeenLastCalledWith(
      "activate_clipboard_history",
      { id: "history-text" },
    );

    fireEvent.click(screen.getByRole("button", { name: "清空" }));
    await flushPromises();
    expect(tauriMocks.invoke).toHaveBeenLastCalledWith(
      "clear_clipboard_history",
    );
  });

  it("keeps the storage fallback notice visible until a newer snapshot clears it", async () => {
    const snapshotWithNotice = cloneSnapshot();
    snapshotWithNotice.diagnosticNotice = {
      code: "storageFallback",
      message: "安装目录不可写，日志已切换到备用目录",
      path: "C:\\Users\\operator\\AppData\\Local\\kvmFlow",
    };
    tauriMocks.invoke.mockResolvedValueOnce(snapshotWithNotice);

    render(<App />);
    await flushPromises();

    expect(screen.getByText("安装目录不可写，日志已切换到备用目录")).toBeVisible();
    expect(
      screen.getByText("C:\\Users\\operator\\AppData\\Local\\kvmFlow"),
    ).toBeVisible();

    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: cloneSnapshot() });
    });

    expect(
      screen.queryByText("安装目录不可写，日志已切换到备用目录"),
    ).not.toBeInTheDocument();
  });

  it("shows a concise Chinese error when the initial snapshot request rejects", async () => {
    tauriMocks.invoke.mockRejectedValueOnce(new Error("bridge unavailable"));

    render(<App />);
    await flushPromises();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(screen.getByRole("alert")).toHaveTextContent("无法加载状态");
    expect(tauriMocks.invoke).toHaveBeenCalledTimes(2);
  });

  it("shows a concise Chinese error when the snapshot listener rejects", async () => {
    let resolveInitialSnapshot: ((snapshot: AppSnapshot) => void) | undefined;
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return new Promise<AppSnapshot>((resolve) => {
          resolveInitialSnapshot = resolve;
        });
      }
      if (command === "list_displays") {
        return Promise.resolve(availableDisplays);
      }

      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    tauriMocks.listen.mockRejectedValueOnce(new Error("listener unavailable"));

    render(<App />);
    await flushPromises();

    expect(screen.getByRole("alert")).toHaveTextContent("无法连接状态更新");

    resolveInitialSnapshot?.(cloneSnapshot());
    await flushPromises();
    openDiagnostics();

    expect(screen.getByText("fixture-build")).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent("无法连接状态更新");
  });

  it("shows a concise Chinese error for a malformed initial snapshot payload", async () => {
    tauriMocks.invoke.mockResolvedValueOnce({ schemaVersion: 1 });

    render(<App />);
    await flushPromises();

    expect(screen.getByRole("alert")).toHaveTextContent("无法加载状态");
  });

  it("shows a concise Chinese error for a malformed snapshot event without crashing", async () => {
    render(<App />);
    await flushPromises();

    expect(() => {
      act(() => {
        eventHandlers.get("engine://snapshot")?.({ payload: { schemaVersion: 1 } });
      });
    }).not.toThrow();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(screen.getByRole("alert")).toHaveTextContent("收到无效状态数据");
    expect(tauriMocks.invoke).toHaveBeenCalledTimes(2);
  });

  it("unlistens when an async listener resolves after unmount", async () => {
    let resolveListener: ((value: typeof snapshotUnlisten) => void) | undefined;
    tauriMocks.listen.mockImplementationOnce(
      () =>
        new Promise<typeof snapshotUnlisten>((resolve) => {
          resolveListener = resolve;
        }),
    );

    const { unmount } = render(<App />);
    unmount();

    resolveListener?.(snapshotUnlisten);
    await flushPromises();

    expect(snapshotUnlisten).toHaveBeenCalledTimes(1);
  });
});
