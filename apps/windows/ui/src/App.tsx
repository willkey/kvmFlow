import { useEffect, useMemo, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { occupiedDisplayEdges } from "@kvmflow/ui-contracts";
import type {
  AppSnapshot,
  DisplayTarget,
  GateStatus,
  MetricProgress,
  ScreenEdge,
} from "@kvmflow/ui-contracts";
import {
  activateClipboardHistory,
  clearClipboardHistory,
  confirmPairing,
  configureScreenEdge,
  configureTargetDisplay,
  connectManual,
  enterControl,
  exportFoundationReport,
  getInitialSnapshot,
  listenForSnapshots,
  listDisplays,
  rejectPairing,
  setControlEnabled,
  setGameModeEnabled,
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

const screenEdgeOptions: Array<{ edge: ScreenEdge; label: string }> = [
  { edge: "left", label: "左侧" },
  { edge: "right", label: "右侧" },
  { edge: "top", label: "上方" },
  { edge: "bottom", label: "下方" },
];

const clipboardKindLabels: Record<
  AppSnapshot["kvm"]["clipboard"]["history"][number]["kind"],
  string
> = {
  text: "文本",
  html: "HTML",
  richText: "富文本",
  image: "图片",
  files: "文件",
  mixed: "混合内容",
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

function displayLabel(display: DisplayTarget): string {
  const id = display.id.replace(/^\\\\\.\\/, "");
  const role = display.primary ? "主显示器" : "显示器";
  return `${role} ${id} · ${display.width} × ${display.height}`;
}

export default function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [listenerErrorMessage, setListenerErrorMessage] = useState<string | null>(null);
  const [displayErrorMessage, setDisplayErrorMessage] = useState<string | null>(null);
  const [displays, setDisplays] = useState<DisplayTarget[]>([]);
  const [reportPath, setReportPath] = useState<string | null>(null);
  const [isExporting, setIsExporting] = useState(false);
  const [ipDraft, setIpDraft] = useState("");
  const [portDraft, setPortDraft] = useState("45888");
  const [isCommandPending, setIsCommandPending] = useState(false);
  const seededConnectionForm = useRef(false);

  useEffect(() => {
    let isCurrent = true;
    let snapshotUnlisten: UnlistenFn | undefined;
    let receivedSnapshotSinceInitialRequest = false;
    let displayRequestSequence = 0;

    const refreshDisplays = () => {
      const requestSequence = ++displayRequestSequence;
      void listDisplays()
        .then((availableDisplays) => {
          if (isCurrent && requestSequence === displayRequestSequence) {
            setDisplays(availableDisplays);
            setDisplayErrorMessage(null);
          }
        })
        .catch(() => {
          if (isCurrent && requestSequence === displayRequestSequence) {
            setDisplayErrorMessage("无法读取 Windows 显示器");
          }
        });
    };

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

    refreshDisplays();

    void listenForSnapshots(
      (nextSnapshot) => {
        receivedSnapshotSinceInitialRequest = true;
        applySnapshot(nextSnapshot);
        refreshDisplays();
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

    return () => {
      isCurrent = false;
      snapshotUnlisten?.();
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

  const selectedDisplayId =
    snapshot?.kvm.targetDisplayId ??
    displays.find((display) => display.primary)?.id ??
    displays[0]?.id ??
    null;
  const occupiedEdges = useMemo(
    () => new Set(occupiedDisplayEdges(displays, selectedDisplayId)),
    [displays, selectedDisplayId],
  );
  const selectedEdgeOccupied =
    snapshot !== null && occupiedEdges.has(snapshot.kvm.screenEdge);
  const resource = snapshot?.latestResource;
  const kvmValidation = snapshot?.validation.kvm;
  const status = snapshot ? windowsStatus(snapshot) : "正在加载";
  const displayedErrorMessage = displayErrorMessage ?? listenerErrorMessage ?? errorMessage;

  return (
    <main className="app-shell" aria-labelledby="app-title">
      <header className="app-header">
        <div className="app-heading">
          <img className="brand-mark" src={brandMark} alt="" />
          <div>
            <p className="app-name">kvmFlow <span>Windows 控制端</span></p>
            <h1 id="app-title">{status}</h1>
          </div>
        </div>
        <div className="header-actions">
          <span className={`link-badge link-badge--${snapshot?.kvm.link ?? "idle"}`}>
            <i aria-hidden="true" />
            {snapshot?.kvm.link === "online" ? "已连接" : "未连接"}
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

      {displayedErrorMessage ? (
        <p className="error-band" role="alert">
          {displayedErrorMessage}
        </p>
      ) : null}

      {snapshot && snapshot.kvm.consecutiveFailures >= 3 ? (
        <p className="reconnect-alert" role="alert">
          已连续重连失败 {snapshot.kvm.consecutiveFailures} 次，程序仍会每 5 秒重试
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

        {snapshot ? (
          <div className="control-preferences">
            <label className="control-toggle">
              <span>
                <strong>启用跨屏控制</strong>
                <small>关闭后保留连接和剪贴板同步，不捕获键盘或屏幕边缘。</small>
              </span>
              <input
                type="checkbox"
                checked={snapshot.kvm.controlEnabled}
                onChange={(event) =>
                  void runCommand(() => setControlEnabled(event.currentTarget.checked))
                }
                disabled={isCommandPending}
              />
            </label>
            <label className="control-toggle">
              <span>
                <strong>前台全屏保护</strong>
                <small>
                  {snapshot.kvm.gameModeEnabled && snapshot.kvm.foregroundFullscreen
                    ? "保护中：前台应用占满显示器，键鼠不会跨屏。"
                    : "全屏应用位于前台时阻止键鼠跨屏，切到后台后自动恢复。"}
                </small>
              </span>
              <input
                type="checkbox"
                checked={snapshot.kvm.gameModeEnabled}
                onChange={(event) =>
                  void runCommand(() => setGameModeEnabled(event.currentTarget.checked))
                }
                disabled={isCommandPending}
              />
            </label>
          </div>
        ) : null}

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
            {displays.length > 0 ? (
              <label className="display-target-field">
                进入 Mac 的 Windows 显示器
                <select
                  value={selectedDisplayId ?? ""}
                  onChange={(event) =>
                    void runCommand(() =>
                      configureTargetDisplay(event.currentTarget.value),
                    )
                  }
                  disabled={isCommandPending || snapshot.kvm.control !== "local"}
                >
                  {displays.map((display) => (
                    <option key={display.id} value={display.id}>
                      {displayLabel(display)}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
            <fieldset className="screen-position-fieldset">
              <legend>Mac 相对位置</legend>
              <div className="segmented-control">
                {screenEdgeOptions.map(({ edge, label }) => (
                  <label key={edge}>
                    <input
                      type="radio"
                      name="screen-edge"
                      value={edge}
                      checked={snapshot.kvm.screenEdge === edge}
                      onChange={() => void runCommand(() => configureScreenEdge(edge))}
                      disabled={
                        isCommandPending ||
                        snapshot.kvm.control !== "local" ||
                        occupiedEdges.has(edge)
                      }
                    />
                    <span>{label}</span>
                  </label>
                ))}
              </div>
              {occupiedEdges.size > 0 ? (
                <p className="screen-position-note" role="status">
                  {selectedEdgeOccupied
                    ? "当前位置与 Windows 显示器重叠，请选择未占用的方向"
                    : "灰色方向已有 Windows 显示器，不能用于进入 Mac"}
                </p>
              ) : null}
            </fieldset>
            {snapshot.kvm.control !== "remote" ? (
              <div className="command-row">
                <button
                  type="button"
                  onClick={() =>
                    void runCommand(() => enterControl(snapshot.kvm.screenEdge))
                  }
                  disabled={
                    isCommandPending ||
                    !snapshot.kvm.controlEnabled ||
                    !snapshot.kvm.remoteControlAvailable ||
                    (snapshot.kvm.gameModeEnabled &&
                      snapshot.kvm.foregroundFullscreen) ||
                    snapshot.kvm.control === "enteringRemote" ||
                    selectedEdgeOccupied
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

      {snapshot ? (
        <details className="clipboard-drawer">
          <summary>
            <span>
              <strong>剪贴板历史</strong>
              <small>
                {clipboardTransferLabels[snapshot.kvm.clipboard.transferState]} · 已保留{" "}
                {snapshot.kvm.clipboard.history.length} / 20
              </small>
            </span>
            <button
              className="history-clear-command"
              type="button"
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                void runCommand(clearClipboardHistory);
              }}
              disabled={
                isCommandPending || snapshot.kvm.clipboard.history.length === 0
              }
            >
              清空
            </button>
          </summary>
          <div className="clipboard-content">
            {snapshot.kvm.clipboard.lastError ? (
              <p className="clipboard-error" role="alert">
                {snapshot.kvm.clipboard.lastError}
              </p>
            ) : null}
            {snapshot.kvm.clipboard.history.length > 0 ? (
              <ol className="clipboard-history-list">
                {snapshot.kvm.clipboard.history.map((item) => (
                  <li key={item.id}>
                    <div className="clipboard-history-copy">
                      <strong>{item.preview}</strong>
                      <span>
                        {clipboardKindLabels[item.kind]} ·{" "}
                        {formatClipboardSize(item.sizeBytes)} ·{" "}
                        {formatClipboardTime(item.capturedAtUnixMs)}
                      </span>
                    </div>
                    <button
                      type="button"
                      onClick={() =>
                        void runCommand(() => activateClipboardHistory(item.id))
                      }
                      disabled={isCommandPending || !item.available}
                      title={
                        item.available
                          ? "将这条内容重新放入两台设备的剪贴板"
                          : "原文件已移动或删除"
                      }
                    >
                      {item.available ? "重新复制" : "文件不可用"}
                    </button>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="clipboard-empty">复制内容后会自动出现在这里。</p>
            )}
          </div>
        </details>
      ) : null}

      <details className="diagnostics-drawer">
        <summary>
          <span>运行与诊断</span>
          <small>验收进度、资源占用与构建信息</small>
        </summary>
        <div className="diagnostics-content">
      <section className="validation-section" aria-labelledby="kvm-validation-heading">
        <div className="section-heading">
          <h2 id="kvm-validation-heading">KVM 验收进度</h2>
          <span className="runtime-state">
            完整切换 {Math.min(kvmValidation?.successfulEntries ?? 0, kvmValidation?.successfulReturns ?? 0)} / 100
          </span>
        </div>
        <dl className="metrics-grid">
          <div className="metric">
            <dt>进入 / 返回</dt>
            <dd>
              {kvmValidation?.successfulEntries ?? 0} / {kvmValidation?.successfulReturns ?? 0}
            </dd>
          </div>
          <div className="metric">
            <dt>切换失败</dt>
            <dd>{kvmValidation?.transitionFailures ?? 0}</dd>
          </div>
          <div className="metric">
            <dt>紧急返回</dt>
            <dd>
              {kvmValidation?.emergencyReturns ?? 0} · {formatLatency(kvmValidation?.peakEmergencyReturnMs)}
            </dd>
          </div>
          <div className="metric">
            <dt>断线 / 重连</dt>
            <dd>
              {kvmValidation?.disconnects ?? 0} / {kvmValidation?.reconnects ?? 0}
            </dd>
          </div>
          <div className="metric">
            <dt>网络 / 唤醒恢复</dt>
            <dd>
              {kvmValidation?.networkRecoveries ?? 0} / {kvmValidation?.wakeRecoveries ?? 0}
            </dd>
          </div>
          <div className="metric">
            <dt>延迟 P95</dt>
            <dd>{formatLatency(kvmValidation?.p95LatencyMs)}</dd>
          </div>
          <div className="metric">
            <dt>延迟平均 / 峰值</dt>
            <dd>
              {formatLatency(kvmValidation?.averageLatencyMs)} / {formatLatency(kvmValidation?.peakLatencyMs)}
            </dd>
          </div>
        </dl>
      </section>

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

function initialMetricProgress(unit: string): MetricProgress {
  return {
    status: "notObserved",
    current: null,
    target: 0,
    unit,
  };
}

function formatLatency(milliseconds: number | null | undefined): string {
  return milliseconds === null || milliseconds === undefined
    ? "--"
    : `${milliseconds.toFixed(1)} ms`;
}

function formatClipboardSize(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1_048_576) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  if (bytes < 1_073_741_824) {
    return `${(bytes / 1_048_576).toFixed(1)} MB`;
  }
  return `${(bytes / 1_073_741_824).toFixed(1)} GB`;
}

function formatClipboardTime(unixMilliseconds: number): string {
  return new Date(unixMilliseconds).toLocaleString("zh-CN", {
    hour12: false,
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
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
