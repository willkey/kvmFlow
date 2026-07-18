import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSnapshot,
  GateStatus,
  KeyboardMonitorState,
  MetricProgress,
  ScreenEdge,
} from "@kvmflow/ui-contracts";
import {
  confirmPairing,
  configureScreenEdge,
  connectManual,
  enterControl,
  exportFoundationReport,
  getInitialSnapshot,
  listenForKeyboardMonitor,
  listenForSnapshots,
  rejectPairing,
} from "./engineBridge";
import KeyboardMonitor, { emptyKeyboardMonitor } from "./KeyboardMonitor";
import "./styles.css";

const runtimeStatus: Record<AppSnapshot["phase"], string> = {
  starting: "基础框架启动中",
  running: "基础框架运行中",
  stopping: "基础框架正在停止",
};

const gateLabels: Record<GateStatus, string> = {
  notObserved: "未观测",
  collecting: "采集中",
  passed: "已通过",
  failed: "未通过",
  manualConfirmationRequired: "需要人工确认",
};

const screenEdgeOptions: Array<{ edge: ScreenEdge; label: string }> = [
  { edge: "left", label: "左侧" },
  { edge: "right", label: "右侧" },
  { edge: "top", label: "上方" },
  { edge: "bottom", label: "下方" },
];

interface GateRowProps {
  label: string;
  progress: MetricProgress;
}

function GateRow({ label, progress }: GateRowProps) {
  return (
    <li className="gate-row">
      <span className="gate-name">{label}</span>
      <span className="gate-progress">{formatGateProgress(progress)}</span>
      <span className={`gate-status gate-status--${progress.status}`}>
        {gateLabels[progress.status]}
      </span>
    </li>
  );
}

function formatGateProgress(progress: MetricProgress): string {
  if (progress.current === null) {
    return "--";
  }

  if (progress.unit === "bytes") {
    return `${formatMemory(progress.current)} / ${formatMemory(progress.target)}`;
  }

  if (progress.unit === "percent") {
    return `${formatCpu(progress.current)} / ${formatCpu(progress.target)}`;
  }

  return `${progress.current} / ${progress.target}`;
}

function formatMemory(bytes: number): string {
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

function formatCpu(cpuPercent: number): string {
  return `${cpuPercent.toFixed(2)}%`;
}

function formatDuration(seconds: number): string {
  if (seconds < 60) {
    return `${seconds} 秒`;
  }
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return hours > 0 ? `${hours} 小时 ${minutes} 分` : `${minutes} 分`;
}

export default function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [keyboardMonitor, setKeyboardMonitor] =
    useState<KeyboardMonitorState>(emptyKeyboardMonitor);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [listenerErrorMessage, setListenerErrorMessage] = useState<string | null>(null);
  const [keyboardErrorMessage, setKeyboardErrorMessage] = useState<string | null>(null);
  const [reportPath, setReportPath] = useState<string | null>(null);
  const [isExporting, setIsExporting] = useState(false);
  const [ipDraft, setIpDraft] = useState("");
  const [portDraft, setPortDraft] = useState("45888");
  const [isCommandPending, setIsCommandPending] = useState(false);
  const seededConnectionForm = useRef(false);

  useEffect(() => {
    let isCurrent = true;
    let snapshotUnlisten: UnlistenFn | undefined;
    let keyboardUnlisten: UnlistenFn | undefined;
    let receivedSnapshotSinceInitialRequest = false;

    const applySnapshot = (nextSnapshot: AppSnapshot) => {
      if (!isCurrent) {
        return;
      }

      if (!seededConnectionForm.current) {
        seededConnectionForm.current = true;
        if (nextSnapshot.kvm.endpoint) {
          setIpDraft(nextSnapshot.kvm.endpoint.ip);
          setPortDraft(String(nextSnapshot.kvm.endpoint.port));
        }
      }
      setSnapshot(nextSnapshot);
      setErrorMessage(null);
    };

    void getInitialSnapshot()
      .then((initialSnapshot) => {
        if (!receivedSnapshotSinceInitialRequest) {
          applySnapshot(initialSnapshot);
        }
      })
      .catch(() => {
        if (isCurrent && !receivedSnapshotSinceInitialRequest) {
          setErrorMessage("无法加载状态");
        }
      });

    void listenForSnapshots(
      (nextSnapshot) => {
        receivedSnapshotSinceInitialRequest = true;
        applySnapshot(nextSnapshot);
      },
      () => {
        if (isCurrent) {
          setErrorMessage("收到无效状态数据");
        }
      },
    )
      .then((release) => {
        if (!isCurrent) {
          release();
          return;
        }

        snapshotUnlisten = release;
      })
      .catch(() => {
        if (isCurrent) {
          setListenerErrorMessage("无法连接状态更新");
        }
      });

    void listenForKeyboardMonitor(
      (monitor) => {
        if (isCurrent) {
          setKeyboardMonitor(monitor);
          setKeyboardErrorMessage(null);
        }
      },
      () => {
        if (isCurrent) {
          setKeyboardErrorMessage("收到无效键盘状态数据");
        }
      },
    )
      .then((release) => {
        if (!isCurrent) {
          release();
          return;
        }

        keyboardUnlisten = release;
      })
      .catch(() => {
        if (isCurrent) {
          setKeyboardErrorMessage("无法连接键盘状态更新");
        }
      });

    return () => {
      isCurrent = false;
      snapshotUnlisten?.();
      keyboardUnlisten?.();
    };
  }, []);

  const handleExport = async () => {
    setIsExporting(true);

    try {
      setReportPath(await exportFoundationReport());
    } catch {
      setErrorMessage("无法导出报告");
    } finally {
      setIsExporting(false);
    }
  };

  const runCommand = async (command: () => Promise<void>) => {
    setIsCommandPending(true);
    try {
      await command();
      setErrorMessage(null);
    } catch (error) {
      setErrorMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCommandPending(false);
    }
  };

  const handleConnect = () =>
    runCommand(async () => {
      const port = Number(portDraft);
      if (!Number.isInteger(port) || port < 1024 || port > 65_535) {
        throw new Error("端口必须在 1024-65535 范围内");
      }
      await connectManual(ipDraft.trim(), port);
    });

  const resource = snapshot?.latestResource;
  const status = snapshot ? windowsStatus(snapshot) : "正在加载";
  const displayedErrorMessage =
    keyboardErrorMessage ?? listenerErrorMessage ?? errorMessage;

  return (
    <main className="app-shell" aria-labelledby="app-title">
      <header className="app-header">
        <div className="app-heading">
          <p className="app-name">kvmFlow · Windows</p>
          <h1 id="app-title">{status}</h1>
        </div>
        <button
          className="export-command"
          type="button"
          onClick={handleExport}
          disabled={isExporting}
        >
          导出基线报告
        </button>
      </header>

      {snapshot?.diagnosticNotice ? (
        <aside className="notice-band" aria-label="诊断提示">
          <strong>{snapshot.diagnosticNotice.message}</strong>
          <code>{snapshot.diagnosticNotice.path ?? "--"}</code>
        </aside>
      ) : null}

      {displayedErrorMessage ? (
        <p className="error-band" role="alert">
          {displayedErrorMessage}
        </p>
      ) : null}

      {reportPath ? (
        <output className="report-path" aria-live="polite">
          {reportPath}
        </output>
      ) : null}

      <section className="connection-section" aria-labelledby="connection-heading">
        <div className="section-heading">
          <h2 id="connection-heading">设备连接</h2>
          <span className={`connection-state connection-state--${snapshot?.kvm.link ?? "idle"}`}>
            {status}
          </span>
        </div>

        {snapshot?.kvm.pairing ? (
          <div className="pairing-panel">
            <p>请确认两台设备显示相同数字</p>
            <strong className="pairing-code">{snapshot.kvm.pairing.code}</strong>
            <p className="pairing-progress">
              本机{snapshot.kvm.pairing.localConfirmed ? "已确认" : "待确认"} · Mac
              {snapshot.kvm.pairing.remoteConfirmed ? "已确认" : "待确认"}
            </p>
            <div className="command-row">
              <button
                type="button"
                onClick={() => void runCommand(confirmPairing)}
                disabled={isCommandPending || snapshot.kvm.pairing.localConfirmed}
              >
                确认配对
              </button>
              <button
                className="secondary-command"
                type="button"
                onClick={() => void runCommand(rejectPairing)}
                disabled={isCommandPending}
              >
                拒绝
              </button>
            </div>
          </div>
        ) : snapshot?.kvm.link === "online" ? (
          <div className="online-panel">
            <dl className="connection-metrics">
              <div>
                <dt>设备</dt>
                <dd>{snapshot.kvm.peerDeviceName ?? "Mac"}</dd>
              </div>
              <div>
                <dt>地址</dt>
                <dd>
                  {snapshot.kvm.endpoint
                    ? `${snapshot.kvm.endpoint.ip}:${snapshot.kvm.endpoint.port}`
                    : "--"}
                </dd>
              </div>
              <div>
                <dt>延迟</dt>
                <dd>
                  {snapshot.kvm.latencyMs === null
                    ? "--"
                    : `${snapshot.kvm.latencyMs.toFixed(1)} ms`}
                </dd>
              </div>
            </dl>
            <fieldset className="screen-position-fieldset">
              <legend>Mac 屏幕位置</legend>
              <div className="segmented-control">
                {screenEdgeOptions.map(({ edge, label }) => (
                  <label key={edge}>
                    <input
                      type="radio"
                      name="screen-edge"
                      value={edge}
                      checked={snapshot.kvm.screenEdge === edge}
                      onChange={() => void runCommand(() => configureScreenEdge(edge))}
                      disabled={isCommandPending || snapshot.kvm.control !== "local"}
                    />
                    <span>{label}</span>
                  </label>
                ))}
              </div>
            </fieldset>
            {snapshot.kvm.control !== "remote" ? (
              <div className="command-row">
                <button
                  type="button"
                  onClick={() =>
                    void runCommand(() => enterControl(snapshot.kvm.screenEdge))
                  }
                  disabled={
                    isCommandPending || snapshot.kvm.control === "enteringRemote"
                  }
                >
                  {snapshot.kvm.control === "enteringRemote"
                    ? "正在开始控制"
                    : "开始控制"}
                </button>
              </div>
            ) : null}
          </div>
        ) : (
          <form
            className="connection-form"
            onSubmit={(event) => {
              event.preventDefault();
              void handleConnect();
            }}
          >
            <label>
              Mac IPv4 地址
              <input
                value={ipDraft}
                onChange={(event) => setIpDraft(event.currentTarget.value)}
                placeholder="例如 192.168.1.20"
                inputMode="decimal"
                autoComplete="off"
              />
            </label>
            <label>
              端口
              <input
                value={portDraft}
                onChange={(event) => setPortDraft(event.currentTarget.value)}
                inputMode="numeric"
                autoComplete="off"
              />
            </label>
            <button type="submit" disabled={isCommandPending || !ipDraft.trim()}>
              {snapshot?.kvm.link === "connecting" ? "正在连接" : "连接"}
            </button>
          </form>
        )}

        {snapshot?.kvm.lastError ? (
          <div className="connection-error" role="alert">
            <strong>
              {snapshot.kvm.lastError.code} · {snapshot.kvm.lastError.message}
            </strong>
            <span>{snapshot.kvm.lastError.recovery}</span>
          </div>
        ) : null}
      </section>

      <KeyboardMonitor monitor={keyboardMonitor} caption="映射后发送" />

      <section className="status-section" aria-labelledby="metrics-heading">
        <div className="section-heading">
          <h2 id="metrics-heading">运行概览</h2>
          <span className="runtime-state" aria-live="polite">
            采样 {snapshot?.validation.sampleCount ?? 0} 次 · CPU 有效
            {snapshot?.validation.validCpuSampleCount ?? 0} 次
          </span>
        </div>
        <dl className="metrics-grid">
          <div className="metric">
            <dt>构建 ID</dt>
            <dd>{snapshot?.build.buildId ?? "--"}</dd>
          </div>
          <div className="metric">
            <dt>进程数</dt>
            <dd>{resource?.processCount ?? "--"}</dd>
          </div>
          <div className="metric">
            <dt>当前 CPU</dt>
            <dd>{resource?.cpuValid ? formatCpu(resource.cpuPercent) : "--"}</dd>
          </div>
          <div className="metric">
            <dt>当前内存</dt>
            <dd>{resource ? formatMemory(resource.residentMemoryBytes) : "--"}</dd>
          </div>
          <div className="metric">
            <dt>平均 CPU</dt>
            <dd>
              {snapshot?.validation.averageCpuPercent === null || !snapshot
                ? "--"
                : formatCpu(snapshot.validation.averageCpuPercent)}
            </dd>
          </div>
          <div className="metric">
            <dt>峰值 CPU</dt>
            <dd>
              {snapshot?.validation.peakCpuPercent === null || !snapshot
                ? "--"
                : formatCpu(snapshot.validation.peakCpuPercent)}
            </dd>
          </div>
          <div className="metric">
            <dt>峰值内存</dt>
            <dd>
              {snapshot?.validation.peakMemoryBytes === null || !snapshot
                ? "--"
                : formatMemory(snapshot.validation.peakMemoryBytes)}
            </dd>
          </div>
          <div className="metric">
            <dt>进程运行</dt>
            <dd>{resource ? formatDuration(resource.uptimeSeconds) : "--"}</dd>
          </div>
        </dl>
      </section>

      <section className="gate-section" aria-labelledby="gates-heading">
        <div className="section-heading">
          <h2 id="gates-heading">资源门槛</h2>
          <span className="gate-duration">
            连续运行 {snapshot?.validation.continuousUptimeSeconds ?? 0} 秒
          </span>
        </div>
        <ul className="gate-list">
          <GateRow
            label="后台内存"
            progress={snapshot?.validation.backgroundMemory ?? initialMetricProgress("bytes")}
          />
          <GateRow
            label="窗口内存"
            progress={snapshot?.validation.visibleMemory ?? initialMetricProgress("bytes")}
          />
          <GateRow
            label="后台 CPU"
            progress={snapshot?.validation.backgroundCpu ?? initialMetricProgress("percent")}
          />
        </ul>
      </section>
    </main>
  );
}

function initialMetricProgress(unit: string): MetricProgress {
  return {
    status: "notObserved",
    current: null,
    target: 0,
    unit,
  };
}

function windowsStatus(snapshot: AppSnapshot): string {
  switch (snapshot.kvm.link) {
    case "connecting":
      return "正在连接 Mac";
    case "authenticating":
      return "等待配对确认";
    case "online":
      return snapshot.kvm.control === "remote" ? "正在控制 Mac" : "已连接 Mac";
    case "backoff":
      return "等待自动重连";
    default:
      return runtimeStatus[snapshot.phase] === "基础框架运行中"
        ? "连接 Mac"
        : runtimeStatus[snapshot.phase];
  }
}
