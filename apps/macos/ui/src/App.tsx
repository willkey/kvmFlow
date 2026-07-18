import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSnapshot,
  GateStatus,
  KeyboardMonitorState,
  MetricProgress,
} from "@kvmflow/ui-contracts";
import {
  configureListenPort,
  confirmPairing,
  exportFoundationReport,
  getInitialSnapshot,
  listenForKeyboardMonitor,
  listenForSnapshots,
  openAccessibilitySettings,
  refreshAccessibilityPermission,
  rejectPairing,
  returnControl,
  setLocalControlAllowed,
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

export default function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [keyboardMonitor, setKeyboardMonitor] =
    useState<KeyboardMonitorState>(emptyKeyboardMonitor);
  const [snapshotErrorMessage, setSnapshotErrorMessage] = useState<string | null>(null);
  const [exportErrorMessage, setExportErrorMessage] = useState<string | null>(null);
  const [listenerErrorMessage, setListenerErrorMessage] = useState<string | null>(null);
  const [keyboardErrorMessage, setKeyboardErrorMessage] = useState<string | null>(null);
  const [reportPath, setReportPath] = useState<string | null>(null);
  const [isExporting, setIsExporting] = useState(false);
  const [commandErrorMessage, setCommandErrorMessage] = useState<string | null>(null);
  const [portDraft, setPortDraft] = useState("45888");
  const [isCommandPending, setIsCommandPending] = useState(false);
  const seededPortForm = useRef(false);

  useEffect(() => {
    let isCurrent = true;
    let snapshotUnlisten: UnlistenFn | undefined;
    let keyboardUnlisten: UnlistenFn | undefined;
    let receivedSnapshotSinceInitialRequest = false;

    const applySnapshot = (nextSnapshot: AppSnapshot) => {
      if (!isCurrent) {
        return;
      }

      if (!seededPortForm.current) {
        seededPortForm.current = true;
        if (nextSnapshot.kvm.endpoint) {
          setPortDraft(String(nextSnapshot.kvm.endpoint.port));
        }
      }
      setSnapshot(nextSnapshot);
      setSnapshotErrorMessage(null);
    };

    void getInitialSnapshot()
      .then((initialSnapshot) => {
        if (!receivedSnapshotSinceInitialRequest) {
          applySnapshot(initialSnapshot);
        }
      })
      .catch(() => {
        if (isCurrent && !receivedSnapshotSinceInitialRequest) {
          setSnapshotErrorMessage("无法加载状态");
        }
      });

    void listenForSnapshots(
      (nextSnapshot) => {
        receivedSnapshotSinceInitialRequest = true;
        applySnapshot(nextSnapshot);
      },
      () => {
        if (isCurrent) {
          setSnapshotErrorMessage("收到无效状态数据");
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
    setExportErrorMessage(null);

    try {
      setReportPath(await exportFoundationReport());
    } catch {
      setExportErrorMessage("无法导出报告");
    } finally {
      setIsExporting(false);
    }
  };

  const runCommand = async (command: () => Promise<void>) => {
    setIsCommandPending(true);
    setCommandErrorMessage(null);
    try {
      await command();
    } catch (error) {
      setCommandErrorMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCommandPending(false);
    }
  };

  const handlePortUpdate = () =>
    runCommand(async () => {
      const port = Number(portDraft);
      if (!Number.isInteger(port) || port < 1024 || port > 65_535) {
        throw new Error("端口必须在 1024-65535 范围内");
      }
      await configureListenPort(port);
    });

  const resource = snapshot?.latestResource;
  const status = snapshot ? macStatus(snapshot) : "正在加载";
  const errorMessages = [
    { source: "listener", message: listenerErrorMessage },
    { source: "keyboard", message: keyboardErrorMessage },
    { source: "snapshot", message: snapshotErrorMessage },
    { source: "export", message: exportErrorMessage },
    { source: "command", message: commandErrorMessage },
  ].filter(({ message }) => message !== null);

  return (
    <main className="app-shell" aria-labelledby="app-title">
      <header className="app-header">
        <div className="app-heading">
          <p className="app-name">kvmFlow · Mac</p>
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

      {errorMessages.length > 0 ? (
        <div className="error-band" role="alert">
          {errorMessages.map(({ source, message }) => (
            <span key={source}>{message}</span>
          ))}
        </div>
      ) : null}

      {reportPath ? (
        <output className="report-path" aria-live="polite">
          {reportPath}
        </output>
      ) : null}

      <section className="connection-section" aria-labelledby="connection-heading">
        <div className="section-heading">
          <h2 id="connection-heading">连接状态</h2>
          <span className={`connection-state connection-state--${snapshot?.kvm.link ?? "idle"}`}>
            {snapshot?.kvm.endpoint
              ? `${snapshot.kvm.endpoint.ip}:${snapshot.kvm.endpoint.port}`
              : "--"}
          </span>
        </div>

        {snapshot?.kvm.pairing ? (
          <div className="pairing-panel">
            <p>请确认两台设备显示相同数字</p>
            <strong className="pairing-code">{snapshot.kvm.pairing.code}</strong>
            <p className="pairing-progress">
              Mac{snapshot.kvm.pairing.localConfirmed ? "已确认" : "待确认"} · Windows
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
                <dt>可信设备</dt>
                <dd>{snapshot.kvm.peerDeviceName ?? "Windows"}</dd>
              </div>
              <div>
                <dt>会话</dt>
                <dd>{snapshot.kvm.sessionId?.slice(0, 12) ?? "--"}</dd>
              </div>
              <div>
                <dt>控制状态</dt>
                <dd>{snapshot.kvm.control === "remote" ? "正在接收控制" : "本机"}</dd>
              </div>
            </dl>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={snapshot.kvm.localPolicy === "allowed"}
                onChange={(event) =>
                  void runCommand(() => setLocalControlAllowed(event.currentTarget.checked))
                }
                disabled={isCommandPending}
              />
              允许远程控制
            </label>
            {snapshot.kvm.control === "remote" ? (
              <div className="command-row">
                <button
                  type="button"
                  onClick={() => void runCommand(returnControl)}
                  disabled={isCommandPending}
                >
                  返回 Windows
                </button>
              </div>
            ) : null}
          </div>
        ) : (
          <form
            className="connection-form connection-form--mac"
            onSubmit={(event) => {
              event.preventDefault();
              void handlePortUpdate();
            }}
          >
            <label>
              TCP 监听端口
              <input
                value={portDraft}
                onChange={(event) => setPortDraft(event.currentTarget.value)}
                inputMode="numeric"
                autoComplete="off"
              />
            </label>
            <button
              type="submit"
              disabled={isCommandPending || snapshot?.kvm.link === "connecting"}
            >
              应用端口
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

        {snapshot && snapshot.kvm.permission !== "granted" ? (
          <div className="permission-panel">
            <div>
              <strong>需要辅助功能权限</strong>
              <span>未授权时可以配对和连接，但不会接收键鼠控制。</span>
            </div>
            <div className="command-row">
              <button
                type="button"
                onClick={() => void runCommand(openAccessibilitySettings)}
                disabled={isCommandPending}
              >
                打开系统设置
              </button>
              <button
                className="secondary-command"
                type="button"
                onClick={() => void runCommand(refreshAccessibilityPermission)}
                disabled={isCommandPending}
              >
                刷新权限
              </button>
            </div>
          </div>
        ) : null}
      </section>

      <KeyboardMonitor monitor={keyboardMonitor} caption="注入前接收" />

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

function initialMetricProgress(unit: string): MetricProgress {
  return {
    status: "notObserved",
    current: null,
    target: 0,
    unit,
  };
}

function macStatus(snapshot: AppSnapshot): string {
  switch (snapshot.kvm.link) {
    case "authenticating":
      return "等待配对确认";
    case "online":
      return snapshot.kvm.control === "remote" ? "运行中" : "已连接 Windows";
    case "portBindFailed":
      return "监听端口不可用";
    case "listening":
      return "等待 Windows 连接";
    default:
      return runtimeStatus[snapshot.phase] === "基础框架运行中"
        ? "正在启动监听"
        : runtimeStatus[snapshot.phase];
  }
}
