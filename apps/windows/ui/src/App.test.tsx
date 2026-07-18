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
  schemaVersion: 2,
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
  },
  kvm: {
    trust: "unpaired",
    link: "idle",
    control: "local",
    permission: "unknown",
    localPolicy: "allowed",
    screenEdge: "bottom",
    endpoint: null,
    peerDeviceName: null,
    pairing: null,
    latencyMs: null,
    sessionId: null,
    lastError: null,
  },
};

function cloneSnapshot(): AppSnapshot {
  return structuredClone(initialSnapshot);
}

async function flushPromises(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("App", () => {
  let eventHandlers: Map<string, TauriEventHandler>;
  let snapshotUnlisten: ReturnType<typeof vi.fn>;
  let keyboardUnlisten: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    eventHandlers = new Map();
    snapshotUnlisten = vi.fn();
    keyboardUnlisten = vi.fn();
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }

      if (command === "export_foundation_report") {
        return Promise.resolve("C:\\kvmFlow\\diagnostics\\foundation-report.json");
      }

      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    tauriMocks.listen.mockImplementation(
      async (eventName: string, callback: TauriEventHandler) => {
        eventHandlers.set(eventName, callback);
        return eventName === "engine://snapshot"
          ? snapshotUnlisten
          : keyboardUnlisten;
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

    expect(tauriMocks.invoke).toHaveBeenCalledTimes(1);
    expect(tauriMocks.invoke).toHaveBeenCalledWith("get_snapshot");
  });

  it("registers snapshot and keyboard listeners and unlistens on unmount", async () => {
    const { unmount } = render(<App />);

    await flushPromises();

    expect(tauriMocks.listen).toHaveBeenCalledTimes(2);
    expect(tauriMocks.listen).toHaveBeenCalledWith(
      "engine://snapshot",
      expect.any(Function),
    );
    expect(tauriMocks.listen).toHaveBeenCalledWith(
      "engine://keyboard",
      expect.any(Function),
    );

    unmount();

    expect(snapshotUnlisten).toHaveBeenCalledTimes(1);
    expect(keyboardUnlisten).toHaveBeenCalledTimes(1);
  });

  it("does not poll after one fake minute", async () => {
    render(<App />);

    await flushPromises();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(tauriMocks.invoke).toHaveBeenCalledTimes(1);
  });

  it("keeps an event snapshot when the pending initial snapshot resolves later", async () => {
    let resolveInitialSnapshot: ((snapshot: AppSnapshot) => void) | undefined;
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return new Promise<AppSnapshot>((resolve) => {
          resolveInitialSnapshot = resolve;
        });
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

  it("exports the foundation report once and renders its path", async () => {
    render(<App />);
    await flushPromises();

    fireEvent.click(screen.getByRole("button", { name: "导出基线报告" }));
    await flushPromises();

    expect(tauriMocks.invoke).toHaveBeenCalledTimes(2);
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

    fireEvent.click(screen.getByRole("radio", { name: "右侧" }));
    await flushPromises();
    expect(tauriMocks.invoke).toHaveBeenLastCalledWith("configure_screen_edge", {
      edge: "right",
    });

    onlineSnapshot.kvm.control = "remote";
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: onlineSnapshot });
    });
    expect(screen.queryByRole("button", { name: "返回本机" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "开始控制" })).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "下方" })).toBeDisabled();
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
    expect(tauriMocks.invoke).toHaveBeenCalledTimes(1);
  });

  it("shows a concise Chinese error when the snapshot listener rejects", async () => {
    let resolveInitialSnapshot: ((snapshot: AppSnapshot) => void) | undefined;
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return new Promise<AppSnapshot>((resolve) => {
          resolveInitialSnapshot = resolve;
        });
      }

      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    tauriMocks.listen.mockRejectedValueOnce(new Error("listener unavailable"));

    render(<App />);
    await flushPromises();

    expect(screen.getByRole("alert")).toHaveTextContent("无法连接状态更新");

    resolveInitialSnapshot?.(cloneSnapshot());
    await flushPromises();

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
    expect(tauriMocks.invoke).toHaveBeenCalledTimes(1);
  });

  it("renders mapped pressed keys and recent keyboard transitions", async () => {
    render(<App />);
    await flushPromises();

    act(() => {
      eventHandlers.get("engine://keyboard")?.({
        payload: {
          sequence: 2,
          pressed: [0xe3, 0x52],
          recent: [
            { sequence: 2, usage: 0x52, action: "down" },
            { sequence: 1, usage: 0xe3, action: "down" },
          ],
        },
      });
    });

    const details = screen.getByText("键盘交互").closest("details");
    expect(details).not.toHaveAttribute("open");
    fireEvent.click(screen.getByText("键盘交互"));
    const keyboard = screen.getByLabelText("Windows 实时键盘状态");
    expect(keyboard.querySelector('[data-usage="227"]')).toHaveAttribute(
      "data-pressed",
      "true",
    );
    expect(keyboard.querySelector('[data-usage="82"]')).toHaveAttribute(
      "data-pressed",
      "true",
    );
    expect(screen.getByText(/映射后发送 · 当前按下 2 个键/)).toBeVisible();
    expect(screen.getAllByText("按下")).toHaveLength(2);
    expect(screen.getAllByText("Ctrl").length).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole("button", { name: "全尺寸键盘" }));
    expect(screen.getByLabelText("数字键盘")).toBeVisible();
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
    expect(keyboardUnlisten).toHaveBeenCalledTimes(1);
  });
});
