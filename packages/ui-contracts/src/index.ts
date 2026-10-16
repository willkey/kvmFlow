export type EndpointRole = "windowsController" | "macEndpoint";

export interface DiscoveredDevice {
  deviceId: string;
  deviceName: string;
  endpoint: NetworkEndpoint;
}

export interface DiscoverySnapshot {
  available: boolean;
  devices: DiscoveredDevice[];
  error: string | null;
}

export function parseDiscoverySnapshot(value: unknown): DiscoverySnapshot {
  const snapshot = requireObject(value, "discovery");
  const devices = requiredField(snapshot, "devices", "discovery");
  if (!Array.isArray(devices) || devices.length > 64) {
    throw new TypeError("discovery.devices must be a bounded array");
  }
  return {
    available: parseBoolean(requiredField(snapshot, "available", "discovery"), "discovery.available"),
    error: parseNullableString(requiredField(snapshot, "error", "discovery"), "discovery.error"),
    devices: devices.map((value, index) => {
      const field = `discovery.devices[${index}]`;
      const device = requireObject(value, field);
      const endpoint = parseNetworkEndpoint(requiredField(device, "endpoint", field), `${field}.endpoint`);
      if (!endpoint) throw new TypeError(`${field}.endpoint cannot be null`);
      return {
        deviceId: parseString(requiredField(device, "deviceId", field), `${field}.deviceId`),
        deviceName: parseString(requiredField(device, "deviceName", field), `${field}.deviceName`),
        endpoint,
      };
    }),
  };
}
export type RuntimePhase = "starting" | "running" | "stopping";
export type WindowState = "open" | "closed";
export type GateStatus =
  | "notObserved"
  | "collecting"
  | "passed"
  | "failed"
  | "manualConfirmationRequired";
export type DiagnosticNoticeCode = "storageFallback";
export type TrustState = "unpaired" | "pairing" | "paired";
export type LinkState =
  | "idle"
  | "listening"
  | "connecting"
  | "authenticating"
  | "online"
  | "backoff"
  | "portBindFailed";
export type ControlState =
  | "local"
  | "enteringRemote"
  | "remote"
  | "returningLocal";
export type PermissionStatus = "unknown" | "missing" | "granted";
export type LocalControlPolicy = "allowed" | "paused";
export type ClipboardContentKind =
  | "text"
  | "html"
  | "richText"
  | "image"
  | "files"
  | "mixed";
export type ClipboardTransferState = "idle" | "sending" | "receiving" | "failed";
export type ScreenEdge = "left" | "right" | "top" | "bottom";
export type ConnectionErrorStage =
  | "portBind"
  | "reachability"
  | "tls"
  | "pairing"
  | "authentication"
  | "permission"
  | "inputAdapter"
  | "protocol";
export type KeyAction = "down" | "up";

export interface BuildInfo {
  version: string;
  buildId: string;
}

export interface DiagnosticNotice {
  code: DiagnosticNoticeCode;
  message: string;
  path: string | null;
}

export interface ResourceSample {
  observedAtUnixMs: number;
  uptimeSeconds: number;
  windowState: WindowState;
  processCount: number;
  residentMemoryBytes: number;
  cpuPercent: number;
  cpuValid: boolean;
}

export interface MetricProgress {
  status: GateStatus;
  current: number | null;
  target: number;
  unit: string;
}

export interface ValidationProgress {
  backgroundMemory: MetricProgress;
  visibleMemory: MetricProgress;
  backgroundCpu: MetricProgress;
  continuousUptimeSeconds: number;
  sampleCount: number;
  validCpuSampleCount: number;
  peakMemoryBytes: number | null;
  averageCpuPercent: number | null;
  peakCpuPercent: number | null;
  kvm: KvmValidationProgress;
}

export interface KvmValidationProgress {
  successfulEntries: number;
  successfulReturns: number;
  transitionFailures: number;
  emergencyReturns: number;
  emergencyReturnSampleCount: number;
  peakEmergencyReturnMs: number | null;
  disconnects: number;
  reconnects: number;
  networkRecoveries: number;
  wakeRecoveries: number;
  latencySampleCount: number;
  averageLatencyMs: number | null;
  p95LatencyMs: number | null;
  peakLatencyMs: number | null;
}

export interface NetworkEndpoint {
  ip: string;
  port: number;
}

export interface DisplayTarget {
  id: string;
  primary: boolean;
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface PairingStatus {
  code: string;
  localConfirmed: boolean;
  remoteConfirmed: boolean;
  expiresAtUnixMs: number;
}

export interface ClipboardHistoryItem {
  id: string;
  capturedAtUnixMs: number;
  kind: ClipboardContentKind;
  preview: string;
  sizeBytes: number;
  available: boolean;
}

export interface ClipboardStatus {
  transferState: ClipboardTransferState;
  lastError: string | null;
  lastUpdatedUnixMs: number | null;
  history: ClipboardHistoryItem[];
}

export interface ConnectionError {
  code: string;
  stage: ConnectionErrorStage;
  message: string;
  recovery: string;
  attemptId: number | null;
  sessionId: string | null;
}

export interface KvmStatus {
  trust: TrustState;
  link: LinkState;
  control: ControlState;
  permission: PermissionStatus;
  localPolicy: LocalControlPolicy;
  controlEnabled: boolean;
  gameModeEnabled: boolean;
  foregroundFullscreen: boolean;
  remoteControlAvailable: boolean;
  clipboard: ClipboardStatus;
  screenEdge: ScreenEdge;
  targetDisplayId: string | null;
  endpoint: NetworkEndpoint | null;
  peerDeviceName: string | null;
  pairing: PairingStatus | null;
  latencyMs: number | null;
  sessionId: string | null;
  consecutiveFailures: number;
  lastError: ConnectionError | null;
}

export interface AppSnapshot {
  schemaVersion: 6;
  role: EndpointRole;
  phase: RuntimePhase;
  windowState: WindowState;
  build: BuildInfo;
  diagnosticNotice: DiagnosticNotice | null;
  latestResource: ResourceSample | null;
  validation: ValidationProgress;
  kvm: KvmStatus;
}

export interface KeyboardTransition {
  sequence: number;
  usage: number;
  action: KeyAction;
}

export interface KeyboardMonitorState {
  sequence: number;
  pressed: number[];
  recent: KeyboardTransition[];
}

export interface KeyboardInteraction {
  sequence: number;
  usages: number[];
  combination: boolean;
  completed: boolean;
}

export function occupiedDisplayEdges(
  displays: DisplayTarget[],
  targetDisplayId: string | null,
): ScreenEdge[] {
  const target = displays.find((display) => display.id === targetDisplayId);
  if (!target) {
    return [];
  }

  const right = target.left + target.width;
  const bottom = target.top + target.height;
  const occupied = new Set<ScreenEdge>();

  for (const display of displays) {
    if (display.id === target.id) {
      continue;
    }

    const displayRight = display.left + display.width;
    const displayBottom = display.top + display.height;
    const verticalOverlap = target.top < displayBottom && display.top < bottom;
    const horizontalOverlap = target.left < displayRight && display.left < right;

    if (displayRight === target.left && verticalOverlap) occupied.add("left");
    if (display.left === right && verticalOverlap) occupied.add("right");
    if (displayBottom === target.top && horizontalOverlap) occupied.add("top");
    if (display.top === bottom && horizontalOverlap) occupied.add("bottom");
  }

  return ["left", "right", "top", "bottom"].filter((edge) =>
    occupied.has(edge as ScreenEdge),
  ) as ScreenEdge[];
}

const modifierUsages = new Set([0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7]);

export function groupKeyboardInteractions(
  monitor: KeyboardMonitorState,
  limit = 8,
): KeyboardInteraction[] {
  const chronological = [...monitor.recent].sort(
    (left, right) => left.sequence - right.sequence,
  );
  const usagesWithTransitions = new Set(chronological.map((transition) => transition.usage));
  const heldModifiers = new Set(
    monitor.pressed.filter(
      (usage) => modifierUsages.has(usage) && !usagesWithTransitions.has(usage),
    ),
  );
  const pendingModifiers = new Map<number, { sequence: number; used: boolean }>();
  const activeKeys = new Map<number, KeyboardInteraction>();
  const interactions: KeyboardInteraction[] = [];

  for (const transition of chronological) {
    if (modifierUsages.has(transition.usage)) {
      if (transition.action === "down") {
        heldModifiers.add(transition.usage);
        if (!pendingModifiers.has(transition.usage)) {
          pendingModifiers.set(transition.usage, {
            sequence: transition.sequence,
            used: false,
          });
        }
      } else {
        heldModifiers.delete(transition.usage);
        const pending = pendingModifiers.get(transition.usage);
        if (pending && !pending.used) {
          interactions.push({
            sequence: pending.sequence,
            usages: [transition.usage],
            combination: false,
            completed: true,
          });
        }
        pendingModifiers.delete(transition.usage);
      }
      continue;
    }

    if (transition.action === "down") {
      if (activeKeys.has(transition.usage)) {
        continue;
      }
      const modifiers = [...heldModifiers];
      for (const modifier of modifiers) {
        const pending = pendingModifiers.get(modifier);
        if (pending) {
          pending.used = true;
        }
      }
      const interaction: KeyboardInteraction = {
        sequence: transition.sequence,
        usages: [...modifiers, transition.usage],
        combination: modifiers.length > 0,
        completed: false,
      };
      activeKeys.set(transition.usage, interaction);
      interactions.push(interaction);
    } else {
      const interaction = activeKeys.get(transition.usage);
      if (interaction) {
        interaction.completed = true;
        activeKeys.delete(transition.usage);
      }
    }
  }

  for (const [usage, pending] of pendingModifiers) {
    if (!pending.used) {
      interactions.push({
        sequence: pending.sequence,
        usages: [usage],
        combination: false,
        completed: false,
      });
    }
  }

  return interactions
    .sort((left, right) => right.sequence - left.sequence)
    .slice(0, Math.max(0, limit));
}

const endpointRoles = new Set<EndpointRole>([
  "windowsController",
  "macEndpoint",
]);
const runtimePhases = new Set<RuntimePhase>(["starting", "running", "stopping"]);
const windowStates = new Set<WindowState>(["open", "closed"]);
const gateStatuses = new Set<GateStatus>([
  "notObserved",
  "collecting",
  "passed",
  "failed",
  "manualConfirmationRequired",
]);
const diagnosticNoticeCodes = new Set<DiagnosticNoticeCode>(["storageFallback"]);
const trustStates = new Set<TrustState>(["unpaired", "pairing", "paired"]);
const linkStates = new Set<LinkState>([
  "idle",
  "listening",
  "connecting",
  "authenticating",
  "online",
  "backoff",
  "portBindFailed",
]);
const controlStates = new Set<ControlState>([
  "local",
  "enteringRemote",
  "remote",
  "returningLocal",
]);
const permissionStatuses = new Set<PermissionStatus>([
  "unknown",
  "missing",
  "granted",
]);
const localControlPolicies = new Set<LocalControlPolicy>(["allowed", "paused"]);
const screenEdges = new Set<ScreenEdge>(["left", "right", "top", "bottom"]);
const clipboardContentKinds = new Set<ClipboardContentKind>([
  "text",
  "html",
  "richText",
  "image",
  "files",
  "mixed",
]);
const clipboardTransferStates = new Set<ClipboardTransferState>([
  "idle",
  "sending",
  "receiving",
  "failed",
]);
const connectionErrorStages = new Set<ConnectionErrorStage>([
  "portBind",
  "reachability",
  "tls",
  "pairing",
  "authentication",
  "permission",
  "inputAdapter",
  "protocol",
]);
const keyActions = new Set<KeyAction>(["down", "up"]);

export function parseKeyboardMonitorState(value: unknown): KeyboardMonitorState {
  const monitor = requireObject(value, "keyboardMonitor");
  const pressed = requiredField(monitor, "pressed", "keyboardMonitor");
  const recent = requiredField(monitor, "recent", "keyboardMonitor");

  if (!Array.isArray(pressed)) {
    throw new TypeError("keyboardMonitor.pressed must be an array");
  }
  if (!Array.isArray(recent)) {
    throw new TypeError("keyboardMonitor.recent must be an array");
  }
  if (recent.length > 12) {
    throw new TypeError("keyboardMonitor.recent must contain at most 12 transitions");
  }

  return {
    sequence: parseUnsignedSafeInteger(
      requiredField(monitor, "sequence", "keyboardMonitor"),
      "keyboardMonitor.sequence",
    ),
    pressed: pressed.map((usage, index) =>
      parseU16(usage, `keyboardMonitor.pressed[${index}]`),
    ),
    recent: recent.map((transition, index) =>
      parseKeyboardTransition(transition, `keyboardMonitor.recent[${index}]`),
    ),
  };
}

function parseKeyboardTransition(
  value: unknown,
  field: string,
): KeyboardTransition {
  const transition = requireObject(value, field);

  return {
    sequence: parseUnsignedSafeInteger(
      requiredField(transition, "sequence", field),
      `${field}.sequence`,
    ),
    usage: parseU16(
      requiredField(transition, "usage", field),
      `${field}.usage`,
    ),
    action: parseEnum(
      requiredField(transition, "action", field),
      keyActions,
      `${field}.action`,
    ),
  };
}

export function parseAppSnapshot(value: unknown): AppSnapshot {
  const snapshot = requireObject(value, "snapshot");

  return {
    schemaVersion: parseSchemaVersion(
      requiredField(snapshot, "schemaVersion", "snapshot"),
      "snapshot.schemaVersion",
    ),
    role: parseEnum(
      requiredField(snapshot, "role", "snapshot"),
      endpointRoles,
      "snapshot.role",
    ),
    phase: parseEnum(
      requiredField(snapshot, "phase", "snapshot"),
      runtimePhases,
      "snapshot.phase",
    ),
    windowState: parseEnum(
      requiredField(snapshot, "windowState", "snapshot"),
      windowStates,
      "snapshot.windowState",
    ),
    build: parseBuildInfo(
      requiredField(snapshot, "build", "snapshot"),
      "snapshot.build",
    ),
    diagnosticNotice: parseDiagnosticNotice(
      requiredField(snapshot, "diagnosticNotice", "snapshot"),
      "snapshot.diagnosticNotice",
    ),
    latestResource: parseResourceSample(
      requiredField(snapshot, "latestResource", "snapshot"),
      "snapshot.latestResource",
    ),
    validation: parseValidationProgress(
      requiredField(snapshot, "validation", "snapshot"),
      "snapshot.validation",
    ),
    kvm: parseKvmStatus(
      requiredField(snapshot, "kvm", "snapshot"),
      "snapshot.kvm",
    ),
  };
}

function parseKvmStatus(value: unknown, field: string): KvmStatus {
  const status = requireObject(value, field);

  return {
    trust: parseEnum(requiredField(status, "trust", field), trustStates, `${field}.trust`),
    link: parseEnum(requiredField(status, "link", field), linkStates, `${field}.link`),
    control: parseEnum(
      requiredField(status, "control", field),
      controlStates,
      `${field}.control`,
    ),
    permission: parseEnum(
      requiredField(status, "permission", field),
      permissionStatuses,
      `${field}.permission`,
    ),
    localPolicy: parseEnum(
      requiredField(status, "localPolicy", field),
      localControlPolicies,
      `${field}.localPolicy`,
    ),
    controlEnabled: parseBoolean(
      requiredField(status, "controlEnabled", field),
      `${field}.controlEnabled`,
    ),
    gameModeEnabled: parseBoolean(
      requiredField(status, "gameModeEnabled", field),
      `${field}.gameModeEnabled`,
    ),
    foregroundFullscreen: parseBoolean(
      requiredField(status, "foregroundFullscreen", field),
      `${field}.foregroundFullscreen`,
    ),
    remoteControlAvailable: parseBoolean(
      requiredField(status, "remoteControlAvailable", field),
      `${field}.remoteControlAvailable`,
    ),
    clipboard: parseClipboardStatus(
      requiredField(status, "clipboard", field),
      `${field}.clipboard`,
    ),
    screenEdge: parseEnum(
      requiredField(status, "screenEdge", field),
      screenEdges,
      `${field}.screenEdge`,
    ),
    targetDisplayId: parseNullableString(
      requiredField(status, "targetDisplayId", field),
      `${field}.targetDisplayId`,
    ),
    endpoint: parseNetworkEndpoint(
      requiredField(status, "endpoint", field),
      `${field}.endpoint`,
    ),
    peerDeviceName: parseNullableString(
      requiredField(status, "peerDeviceName", field),
      `${field}.peerDeviceName`,
    ),
    pairing: parsePairingStatus(
      requiredField(status, "pairing", field),
      `${field}.pairing`,
    ),
    latencyMs: parseNullableFiniteNumber(
      requiredField(status, "latencyMs", field),
      `${field}.latencyMs`,
    ),
    sessionId: parseNullableString(
      requiredField(status, "sessionId", field),
      `${field}.sessionId`,
    ),
    consecutiveFailures: parseU32(
      requiredField(status, "consecutiveFailures", field),
      `${field}.consecutiveFailures`,
    ),
    lastError: parseConnectionError(
      requiredField(status, "lastError", field),
      `${field}.lastError`,
    ),
  };
}

function parseClipboardStatus(value: unknown, field: string): ClipboardStatus {
  const status = requireObject(value, field);
  const history = requiredField(status, "history", field);
  if (!Array.isArray(history)) {
    throw new TypeError(`${field}.history must be an array`);
  }
  return {
    transferState: parseEnum(
      requiredField(status, "transferState", field),
      clipboardTransferStates,
      `${field}.transferState`,
    ),
    lastError: parseNullableString(
      requiredField(status, "lastError", field),
      `${field}.lastError`,
    ),
    lastUpdatedUnixMs: parseNullableUnsignedSafeInteger(
      requiredField(status, "lastUpdatedUnixMs", field),
      `${field}.lastUpdatedUnixMs`,
    ),
    history: history.map((item, index) =>
      parseClipboardHistoryItem(item, `${field}.history[${index}]`),
    ),
  };
}

function parseClipboardHistoryItem(
  value: unknown,
  field: string,
): ClipboardHistoryItem {
  const item = requireObject(value, field);
  return {
    id: parseString(requiredField(item, "id", field), `${field}.id`),
    capturedAtUnixMs: parseUnsignedSafeInteger(
      requiredField(item, "capturedAtUnixMs", field),
      `${field}.capturedAtUnixMs`,
    ),
    kind: parseEnum(
      requiredField(item, "kind", field),
      clipboardContentKinds,
      `${field}.kind`,
    ),
    preview: parseString(
      requiredField(item, "preview", field),
      `${field}.preview`,
    ),
    sizeBytes: parseUnsignedSafeInteger(
      requiredField(item, "sizeBytes", field),
      `${field}.sizeBytes`,
    ),
    available: parseBoolean(
      requiredField(item, "available", field),
      `${field}.available`,
    ),
  };
}

export function parseDisplayTargets(value: unknown): DisplayTarget[] {
  if (!Array.isArray(value)) {
    throw new TypeError("displays must be an array");
  }
  return value.map((display, index) => {
    const field = `displays[${index}]`;
    const target = requireObject(display, field);
    return {
      id: parseString(requiredField(target, "id", field), `${field}.id`),
      primary: parseBoolean(
        requiredField(target, "primary", field),
        `${field}.primary`,
      ),
      left: parseSafeInteger(
        requiredField(target, "left", field),
        `${field}.left`,
      ),
      top: parseSafeInteger(
        requiredField(target, "top", field),
        `${field}.top`,
      ),
      width: parseUnsignedSafeInteger(
        requiredField(target, "width", field),
        `${field}.width`,
      ),
      height: parseUnsignedSafeInteger(
        requiredField(target, "height", field),
        `${field}.height`,
      ),
    };
  });
}

function parseNetworkEndpoint(
  value: unknown,
  field: string,
): NetworkEndpoint | null {
  if (value === null) {
    return null;
  }
  const endpoint = requireObject(value, field);
  return {
    ip: parseString(requiredField(endpoint, "ip", field), `${field}.ip`),
    port: parseU16(requiredField(endpoint, "port", field), `${field}.port`),
  };
}

function parsePairingStatus(
  value: unknown,
  field: string,
): PairingStatus | null {
  if (value === null) {
    return null;
  }
  const pairing = requireObject(value, field);
  return {
    code: parseString(requiredField(pairing, "code", field), `${field}.code`),
    localConfirmed: parseBoolean(
      requiredField(pairing, "localConfirmed", field),
      `${field}.localConfirmed`,
    ),
    remoteConfirmed: parseBoolean(
      requiredField(pairing, "remoteConfirmed", field),
      `${field}.remoteConfirmed`,
    ),
    expiresAtUnixMs: parseUnsignedSafeInteger(
      requiredField(pairing, "expiresAtUnixMs", field),
      `${field}.expiresAtUnixMs`,
    ),
  };
}

function parseConnectionError(
  value: unknown,
  field: string,
): ConnectionError | null {
  if (value === null) {
    return null;
  }
  const error = requireObject(value, field);
  return {
    code: parseString(requiredField(error, "code", field), `${field}.code`),
    stage: parseEnum(
      requiredField(error, "stage", field),
      connectionErrorStages,
      `${field}.stage`,
    ),
    message: parseString(requiredField(error, "message", field), `${field}.message`),
    recovery: parseString(
      requiredField(error, "recovery", field),
      `${field}.recovery`,
    ),
    attemptId: parseNullableUnsignedSafeInteger(
      requiredField(error, "attemptId", field),
      `${field}.attemptId`,
    ),
    sessionId: parseNullableString(
      requiredField(error, "sessionId", field),
      `${field}.sessionId`,
    ),
  };
}

function parseBuildInfo(value: unknown, field: string): BuildInfo {
  const build = requireObject(value, field);

  return {
    version: parseString(requiredField(build, "version", field), `${field}.version`),
    buildId: parseString(requiredField(build, "buildId", field), `${field}.buildId`),
  };
}

function parseDiagnosticNotice(
  value: unknown,
  field: string,
): DiagnosticNotice | null {
  if (value === null) {
    return null;
  }

  const notice = requireObject(value, field);

  return {
    code: parseEnum(
      requiredField(notice, "code", field),
      diagnosticNoticeCodes,
      `${field}.code`,
    ),
    message: parseString(
      requiredField(notice, "message", field),
      `${field}.message`,
    ),
    path: parseNullableString(requiredField(notice, "path", field), `${field}.path`),
  };
}

function parseResourceSample(value: unknown, field: string): ResourceSample | null {
  if (value === null) {
    return null;
  }

  const resource = requireObject(value, field);

  return {
    observedAtUnixMs: parseUnsignedSafeInteger(
      requiredField(resource, "observedAtUnixMs", field),
      `${field}.observedAtUnixMs`,
    ),
    uptimeSeconds: parseUnsignedSafeInteger(
      requiredField(resource, "uptimeSeconds", field),
      `${field}.uptimeSeconds`,
    ),
    windowState: parseEnum(
      requiredField(resource, "windowState", field),
      windowStates,
      `${field}.windowState`,
    ),
    processCount: parseU32(
      requiredField(resource, "processCount", field),
      `${field}.processCount`,
    ),
    residentMemoryBytes: parseUnsignedSafeInteger(
      requiredField(resource, "residentMemoryBytes", field),
      `${field}.residentMemoryBytes`,
    ),
    cpuPercent: parseFiniteNumber(
      requiredField(resource, "cpuPercent", field),
      `${field}.cpuPercent`,
    ),
    cpuValid: parseBoolean(requiredField(resource, "cpuValid", field), `${field}.cpuValid`),
  };
}

function parseValidationProgress(value: unknown, field: string): ValidationProgress {
  const validation = requireObject(value, field);

  return {
    backgroundMemory: parseMetricProgress(
      requiredField(validation, "backgroundMemory", field),
      `${field}.backgroundMemory`,
      true,
    ),
    visibleMemory: parseMetricProgress(
      requiredField(validation, "visibleMemory", field),
      `${field}.visibleMemory`,
      true,
    ),
    backgroundCpu: parseMetricProgress(
      requiredField(validation, "backgroundCpu", field),
      `${field}.backgroundCpu`,
      false,
    ),
    continuousUptimeSeconds: parseUnsignedSafeInteger(
      requiredField(validation, "continuousUptimeSeconds", field),
      `${field}.continuousUptimeSeconds`,
    ),
    sampleCount: parseUnsignedSafeInteger(
      requiredField(validation, "sampleCount", field),
      `${field}.sampleCount`,
    ),
    validCpuSampleCount: parseUnsignedSafeInteger(
      requiredField(validation, "validCpuSampleCount", field),
      `${field}.validCpuSampleCount`,
    ),
    peakMemoryBytes: parseNullableUnsignedSafeInteger(
      requiredField(validation, "peakMemoryBytes", field),
      `${field}.peakMemoryBytes`,
    ),
    averageCpuPercent: parseNullableNonNegativeNumber(
      requiredField(validation, "averageCpuPercent", field),
      `${field}.averageCpuPercent`,
    ),
    peakCpuPercent: parseNullableNonNegativeNumber(
      requiredField(validation, "peakCpuPercent", field),
      `${field}.peakCpuPercent`,
    ),
    kvm: parseKvmValidationProgress(
      requiredField(validation, "kvm", field),
      `${field}.kvm`,
    ),
  };
}

function parseKvmValidationProgress(
  value: unknown,
  field: string,
): KvmValidationProgress {
  const kvm = requireObject(value, field);
  const count = (name: string) =>
    parseUnsignedSafeInteger(requiredField(kvm, name, field), `${field}.${name}`);
  const latency = (name: string) =>
    parseNullableNonNegativeNumber(requiredField(kvm, name, field), `${field}.${name}`);

  return {
    successfulEntries: count("successfulEntries"),
    successfulReturns: count("successfulReturns"),
    transitionFailures: count("transitionFailures"),
    emergencyReturns: count("emergencyReturns"),
    emergencyReturnSampleCount: count("emergencyReturnSampleCount"),
    peakEmergencyReturnMs: latency("peakEmergencyReturnMs"),
    disconnects: count("disconnects"),
    reconnects: count("reconnects"),
    networkRecoveries: count("networkRecoveries"),
    wakeRecoveries: count("wakeRecoveries"),
    latencySampleCount: count("latencySampleCount"),
    averageLatencyMs: latency("averageLatencyMs"),
    p95LatencyMs: latency("p95LatencyMs"),
    peakLatencyMs: latency("peakLatencyMs"),
  };
}

function parseMetricProgress(
  value: unknown,
  field: string,
  isMemoryMetric: boolean,
): MetricProgress {
  const metric = requireObject(value, field);
  const parseMetricNumber = isMemoryMetric
    ? parseNonNegativeNumber
    : parseFiniteNumber;
  const current = requiredField(metric, "current", field);

  return {
    status: parseEnum(
      requiredField(metric, "status", field),
      gateStatuses,
      `${field}.status`,
    ),
    current: current === null ? null : parseMetricNumber(current, `${field}.current`),
    target: parseMetricNumber(
      requiredField(metric, "target", field),
      `${field}.target`,
    ),
    unit: parseString(requiredField(metric, "unit", field), `${field}.unit`),
  };
}

function requireObject(value: unknown, field: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError(`${field} must be an object`);
  }

  return value as Record<string, unknown>;
}

function requiredField(
  value: Record<string, unknown>,
  key: string,
  parentField: string,
): unknown {
  if (!Object.prototype.hasOwnProperty.call(value, key)) {
    throw new TypeError(`${parentField}.${key} is required`);
  }

  return value[key];
}

function parseSchemaVersion(value: unknown, field: string): 6 {
  if (value !== 6) {
    throw new TypeError(`${field} must be 6`);
  }

  return 6;
}

function parseEnum<T extends string>(
  value: unknown,
  allowedValues: ReadonlySet<T>,
  field: string,
): T {
  if (typeof value !== "string" || !allowedValues.has(value as T)) {
    throw new TypeError(`${field} is not a supported value`);
  }

  return value as T;
}

function parseString(value: unknown, field: string): string {
  if (typeof value !== "string") {
    throw new TypeError(`${field} must be a string`);
  }

  return value;
}

function parseNullableString(value: unknown, field: string): string | null {
  return value === null ? null : parseString(value, field);
}

function parseNullableFiniteNumber(value: unknown, field: string): number | null {
  return value === null ? null : parseFiniteNumber(value, field);
}

function parseNullableNonNegativeNumber(value: unknown, field: string): number | null {
  return value === null ? null : parseNonNegativeNumber(value, field);
}

function parseBoolean(value: unknown, field: string): boolean {
  if (typeof value !== "boolean") {
    throw new TypeError(`${field} must be a boolean`);
  }

  return value;
}

function parseFiniteNumber(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`${field} must be a finite number`);
  }

  return value;
}

function parseNonNegativeNumber(value: unknown, field: string): number {
  const number = parseFiniteNumber(value, field);

  if (number < 0) {
    throw new TypeError(`${field} must be nonnegative`);
  }

  return number;
}

function parseUnsignedSafeInteger(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new TypeError(`${field} must be a nonnegative safe integer`);
  }

  return value;
}

function parseSafeInteger(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new TypeError(`${field} must be a safe integer`);
  }

  return value;
}

function parseNullableUnsignedSafeInteger(
  value: unknown,
  field: string,
): number | null {
  return value === null ? null : parseUnsignedSafeInteger(value, field);
}

function parseU16(value: unknown, field: string): number {
  const number = parseUnsignedSafeInteger(value, field);

  if (number > 65_535) {
    throw new TypeError(`${field} must be an unsigned 16-bit integer`);
  }

  return number;
}

function parseU32(value: unknown, field: string): number {
  const number = parseUnsignedSafeInteger(value, field);

  if (number > 4_294_967_295) {
    throw new TypeError(`${field} must be an unsigned 32-bit integer`);
  }

  return number;
}
