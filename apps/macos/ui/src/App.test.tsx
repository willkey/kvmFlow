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
  role: "macEndpoint",
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
    link: "listening",
    control: "local",
    permission: "unknown",
    localPolicy: "allowed",
    screenEdge: "bottom",
    endpoint: { ip: "0.0.0.0", port: 45_888 },
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
        return Promise.resolve("/Applications/kvmFlow/diagnostics/foundation-report.json");
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

  it("renders the Mac listener and local control surface", async () => {
    render(<App />);
    await flushPromises();

    expect(screen.getByText("kvmFlow · Mac")).toBeVisible();
    expect(
      screen.getByRole("heading", { level: 1, name: "等待 Windows 连接" }),
    ).toBeVisible();
    expect(screen.getByText("运行概览")).toBeVisible();
    expect(screen.getByText("资源门槛")).toBeVisible();
    expect(screen.getByText("构建 ID")).toBeVisible();
    expect(screen.getByText("进程数")).toBeVisible();
    expect(screen.getByText("当前 CPU")).toBeVisible();
    expect(screen.getByText("当前内存")).toBeVisible();
    expect(screen.getByText("连接状态")).toBeVisible();
    expect(screen.getByLabelText("TCP 监听端口")).toHaveValue("45888");
    expect(screen.getByRole("button", { name: "应用端口" })).toBeVisible();
    expect(screen.getByRole("button", { name: "导出基线报告" })).toBeVisible();
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
    eventSnapshot.latestResource = resourceSample();

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
    emittedSnapshot.latestResource = resourceSample();
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
      screen.getByText("/Applications/kvmFlow/diagnostics/foundation-report.json"),
    ).toBeVisible();
  });

  it("offers returning control on Mac only while Windows is controlling", async () => {
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }
      if (command === "return_control") {
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await flushPromises();
    expect(screen.queryByRole("button", { name: "返回 Windows" })).not.toBeInTheDocument();

    const remoteSnapshot = cloneSnapshot();
    remoteSnapshot.kvm.link = "online";
    remoteSnapshot.kvm.trust = "paired";
    remoteSnapshot.kvm.permission = "granted";
    remoteSnapshot.kvm.control = "remote";
    remoteSnapshot.kvm.peerDeviceName = "Windows 控制端";
    act(() => {
      eventHandlers.get("engine://snapshot")?.({ payload: remoteSnapshot });
    });

    fireEvent.click(screen.getByRole("button", { name: "返回 Windows" }));
    await flushPromises();
    expect(tauriMocks.invoke).toHaveBeenLastCalledWith("return_control");
  });

  it("keeps the storage fallback notice visible until a newer snapshot clears it", async () => {
    const snapshotWithNotice = cloneSnapshot();
    snapshotWithNotice.diagnosticNotice = {
      code: "storageFallback",
      message: "安装目录不可写，日志已切换到备用目录",
      path: "/Users/operator/Library/Application Support/kvmFlow",
    };
    tauriMocks.invoke.mockResolvedValueOnce(snapshotWithNotice);

    render(<App />);
    await flushPromises();

    expect(screen.getByText("安装目录不可写，日志已切换到备用目录")).toBeVisible();
    expect(
      screen.getByText("/Users/operator/Library/Application Support/kvmFlow"),
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

  it("keeps a listener failure visible after the initial snapshot succeeds", async () => {
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

  it("shows a concise Chinese error when foundation report export rejects", async () => {
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }

      return Promise.reject(new Error("export unavailable"));
    });

    render(<App />);
    await flushPromises();
    fireEvent.click(screen.getByRole("button", { name: "导出基线报告" }));
    await flushPromises();

    expect(screen.getByRole("alert")).toHaveTextContent("无法导出报告");
  });

  it("shows listener and report export failures together", async () => {
    tauriMocks.listen.mockRejectedValueOnce(new Error("listener unavailable"));
    tauriMocks.invoke.mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return Promise.resolve(cloneSnapshot());
      }

      return Promise.reject(new Error("export unavailable"));
    });

    render(<App />);
    await flushPromises();

    expect(screen.getByText("fixture-build")).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent("无法连接状态更新");

    fireEvent.click(screen.getByRole("button", { name: "导出基线报告" }));
    await flushPromises();

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("无法连接状态更新");
    expect(alert).toHaveTextContent("无法导出报告");
  });

  describe("error-source isolation", () => {
    it("keeps a report export failure visible after a valid engine snapshot", async () => {
      tauriMocks.invoke.mockImplementation((command: string) => {
        if (command === "get_snapshot") {
          return Promise.resolve(cloneSnapshot());
        }

        return Promise.reject(new Error("export unavailable"));
      });

      render(<App />);
      await flushPromises();
      fireEvent.click(screen.getByRole("button", { name: "导出基线报告" }));
      await flushPromises();

      expect(screen.getByRole("alert")).toHaveTextContent("无法导出报告");

      const nextSnapshot = cloneSnapshot();
      nextSnapshot.build.buildId = "newer-event-build";
      act(() => {
        eventHandlers.get("engine://snapshot")?.({ payload: nextSnapshot });
      });

      expect(screen.getByText("newer-event-build")).toBeVisible();
      expect(screen.getByRole("alert")).toHaveTextContent("无法导出报告");
    });

    it("clears only the export failure after a successful export retry", async () => {
      let exportAttempts = 0;
      tauriMocks.listen.mockRejectedValueOnce(new Error("listener unavailable"));
      tauriMocks.invoke.mockImplementation((command: string) => {
        if (command === "get_snapshot") {
          return Promise.resolve(cloneSnapshot());
        }

        exportAttempts += 1;
        if (exportAttempts === 1) {
          return Promise.reject(new Error("export unavailable"));
        }

        return Promise.resolve("/Applications/kvmFlow/diagnostics/retry-report.json");
      });

      render(<App />);
      await flushPromises();
      fireEvent.click(screen.getByRole("button", { name: "导出基线报告" }));
      await flushPromises();

      expect(screen.getByRole("alert")).toHaveTextContent("无法连接状态更新");
      expect(screen.getByRole("alert")).toHaveTextContent("无法导出报告");

      fireEvent.click(screen.getByRole("button", { name: "导出基线报告" }));
      await flushPromises();

      const alert = screen.getByRole("alert");
      expect(alert).toHaveTextContent("无法连接状态更新");
      expect(alert).not.toHaveTextContent("无法导出报告");
      expect(
        screen.getByText("/Applications/kvmFlow/diagnostics/retry-report.json"),
      ).toBeVisible();
    });

    it("shows malformed snapshot and export failures together in one alert", async () => {
      tauriMocks.invoke.mockImplementation((command: string) => {
        if (command === "get_snapshot") {
          return Promise.resolve(cloneSnapshot());
        }

        return Promise.reject(new Error("export unavailable"));
      });

      render(<App />);
      await flushPromises();

      act(() => {
        eventHandlers.get("engine://snapshot")?.({ payload: { schemaVersion: 1 } });
      });
      fireEvent.click(screen.getByRole("button", { name: "导出基线报告" }));
      await flushPromises();

      const alerts = screen.getAllByRole("alert");
      expect(alerts).toHaveLength(1);
      expect(alerts[0]).toHaveTextContent("收到无效状态数据");
      expect(alerts[0]).toHaveTextContent("无法导出报告");
    });
  });

  it("renders received pressed keys and recent keyboard transitions", async () => {
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
    const keyboard = screen.getByLabelText("Mac 实时键盘状态");
    expect(keyboard.querySelector('[data-usage="227"]')).toHaveAttribute(
      "data-pressed",
      "true",
    );
    expect(keyboard.querySelector('[data-usage="82"]')).toHaveAttribute(
      "data-pressed",
      "true",
    );
    expect(screen.getByText(/注入前接收 · 当前按下 2 个键/)).toBeVisible();
    expect(screen.getAllByText("按下")).toHaveLength(2);
    expect(screen.getAllByText("command").length).toBeGreaterThan(0);

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

function resourceSample() {
  return {
    observedAtUnixMs: 1_700_000_000_000,
    uptimeSeconds: 42,
    windowState: "open" as const,
    processCount: 4,
    residentMemoryBytes: 67_108_864,
    cpuPercent: 0.32,
    cpuValid: true,
  };
}
