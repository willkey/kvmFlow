import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSnapshot,
  GateStatus,
  MetricProgress,
} from "@kvmflow/ui-contracts";
import {
  configureListenPort,
  confirmPairing,
  exportFoundationReport,
  getAutostartEnabled,
  getInitialSnapshot,
  listenForSnapshots,
  openAccessibilitySettings,
  refreshAccessibilityPermission,
  rejectPairing,
  resetInput,
  returnControl,
  setAutostartEnabled,
  setEmergencyStopActive,
} from "./engineBridge";
import brandMark from "../../../../assets/brand/kvmflow-mark.svg";
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

const clipboardTransferLabels: Record<
  AppSnapshot["kvm"]["clipboard"]["transferState"],
  string
> = {
  idle: "同步就绪",
  sending: "正在发送",
  receiving: "正在接收",
  failed: "同步失败",
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
  const [snapshotErrorMessage, setSnapshotErrorMessage] = useState<string | null>(null);
  const [exportErrorMessage, setExportErrorMessage] = useState<string | null>(null);
  const [listenerErrorMessage, setListenerErrorMessage] = useState<string | null>(null);
  const [preferenceErrorMessage, setPreferenceErrorMessage] = useState<string | null>(null);
  const [reportPath, setReportPath] = useState<string | null>(null);
  const [isExporting, setIsExporting] = useState(false);
  const [commandErrorMessage, setCommandErrorMessage] = useState<string | null>(null);
  const [portDraft, setPortDraft] = useState("45888");
  const [isCommandPending, setIsCommandPending] = useState(false);
  const [resetNotice, setResetNotice] = useState<string | null>(null);
  const [autostartEnabled, setAutostartEnabledState] = useState<boolean | null>(null);
  const seededPortForm = useRef(false);

  useEffect(() => {
    let isCurrent = true;
    let snapshotUnlisten: UnlistenFn | undefined;
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

    void getAutostartEnabled()
      .then((enabled) => {
        if (isCurrent) {
          setAutostartEnabledState(enabled);
          setPreferenceErrorMessage(null);
        }
      })
      .catch(() => {
        if (isCurrent) {
          setPreferenceErrorMessage("无法读取开机自启动设置");
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

    return () => {
      isCurrent = false;
      snapshotUnlisten?.();
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
    setResetNotice(null);
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
    { source: "preference", message: preferenceErrorMessage },
    { source: "snapshot", message: snapshotErrorMessage },
    { source: "export", message: exportErrorMessage },
    { source: "command", message: commandErrorMessage },
  ].filter(({ message }) => message !== null);

  return (
    <main className="app-shell" aria-labelledby="app-title">
      <header className="app-header">
        <div className="app-heading">
          <img className="brand-mark" src={brandMark} alt="" />
          <div>
            <p className="app-name">kvmFlow <span>Mac 被控端</span></p>
            <h1 id="app-title">{status}</h1>
          </div>
        </div>
        <div className="header-actions">
          <button className="export-command reset-command" type="button"
            title="释放双端按键并返回 Windows"
            onClick={() => void runCommand(async () => {
              await resetInput();
              setResetNotice("重置请求已发送，控制会返回 Windows。");
            })} disabled={isCommandPending || !snapshot}>
            重置按键状态
          </button>
          <span className={`link-badge link-badge--${snapshot?.kvm.link ?? "idle"}`}>
            <i aria-hidden="true" />
            {snapshot?.kvm.link === "online" ? "已连接" : "等待连接"}
          </span>
          <button
            className="export-command"
            type="button"
            onClick={handleExport}
            disabled={isExporting}
          >
            导出诊断
          </button>
        </div>
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

      {resetNotice ? <p className="reset-notice" role="status">{resetNotice}</p> : null}

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
              <div>
                <dt>剪贴板</dt>
                <dd>
                  {clipboardTransferLabels[snapshot.kvm.clipboard.transferState]}
                </dd>
              </div>
            </dl>
            {snapshot.kvm.clipboard.lastError ? (
              <p className="clipboard-error" role="alert">
                {snapshot.kvm.clipboard.lastError}
              </p>
            ) : null}
            <div className="local-safety-control">
              <div>
                <strong>本机安全控制</strong>
                <span>
                  {snapshot.kvm.localPolicy === "paused"
                    ? "本机已停止接收控制，Windows 无法远程恢复。"
                    : "遇到异常时可立即释放按键并停止本次控制。"}
                </span>
              </div>
              <div className="command-row">
                {snapshot.kvm.localPolicy === "paused" ? (
                  <button
                    type="button"
                    onClick={() => void runCommand(() => setEmergencyStopActive(false))}
                    disabled={isCommandPending}
                  >
                    恢复等待控制
                  </button>
                ) : (
                  <button
                    className="danger-command"
                    type="button"
                    onClick={() => void runCommand(() => setEmergencyStopActive(true))}
                    disabled={isCommandPending}
                  >
                    立即停止本次控制
                  </button>
                )}
                {snapshot.kvm.control === "remote" ? (
                  <button
                    className="secondary-command"
                    type="button"
                    onClick={() => void runCommand(returnControl)}
                    disabled={isCommandPending}
                  >
                    返回 Windows
                  </button>
                ) : null}
              </div>
            </div>
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

        <label className="autostart-toggle">
          <span>
            <strong>开机自动启动</strong>
            <small>登录 Mac 后在菜单栏后台启动，不主动打开主窗口。</small>
          </span>
          <input
            type="checkbox"
            checked={autostartEnabled ?? false}
            onChange={(event) => {
              const enabled = event.currentTarget.checked;
              void runCommand(async () => {
                setAutostartEnabledState(await setAutostartEnabled(enabled));
                setPreferenceErrorMessage(null);
              });
            }}
            disabled={isCommandPending || autostartEnabled === null}
          />
        </label>

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

      <details className="diagnostics-drawer">
        <summary>
          <span>运行与诊断</span>
          <small>资源占用、运行时长与构建信息</small>
        </summary>
        <div className="diagnostics-content">
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
        </div>
      </details>
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
