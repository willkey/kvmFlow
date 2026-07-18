import { useMemo, useState, type CSSProperties } from "react";
import type { KeyboardMonitorState } from "@kvmflow/ui-contracts";

interface KeyboardMonitorProps {
  monitor: KeyboardMonitorState;
  caption: string;
}

type KeyboardLayoutKind = "compact" | "full";

interface KeyDefinition {
  usage?: number;
  label?: string;
  span?: number;
  rowSpan?: number;
  spacer?: boolean;
}

const mainRows: KeyDefinition[][] = [
  [
    key(0x29, "esc"),
    spacer(),
    key(0x3a, "F1"),
    key(0x3b, "F2"),
    key(0x3c, "F3"),
    key(0x3d, "F4"),
    key(0x3e, "F5"),
    key(0x3f, "F6"),
    key(0x40, "F7"),
    key(0x41, "F8"),
    key(0x42, "F9"),
    key(0x43, "F10"),
    key(0x44, "F11"),
    key(0x45, "F12"),
    spacer(),
  ],
  [
    key(0x35, "`"),
    key(0x1e, "1"),
    key(0x1f, "2"),
    key(0x20, "3"),
    key(0x21, "4"),
    key(0x22, "5"),
    key(0x23, "6"),
    key(0x24, "7"),
    key(0x25, "8"),
    key(0x26, "9"),
    key(0x27, "0"),
    key(0x2d, "-"),
    key(0x2e, "="),
    key(0x2a, "delete", 2),
  ],
  [
    key(0x2b, "tab", 2),
    key(0x14, "Q"),
    key(0x1a, "W"),
    key(0x08, "E"),
    key(0x15, "R"),
    key(0x17, "T"),
    key(0x1c, "Y"),
    key(0x18, "U"),
    key(0x0c, "I"),
    key(0x12, "O"),
    key(0x13, "P"),
    key(0x2f, "["),
    key(0x30, "]"),
    key(0x31, "\\"),
  ],
  [
    key(0x39, "caps lock", 2),
    key(0x04, "A"),
    key(0x16, "S"),
    key(0x07, "D"),
    key(0x09, "F"),
    key(0x0a, "G"),
    key(0x0b, "H"),
    key(0x0d, "J"),
    key(0x0e, "K"),
    key(0x0f, "L"),
    key(0x33, ";"),
    key(0x34, "'"),
    key(0x28, "return", 2),
  ],
  [
    key(0xe1, "shift", 2),
    key(0x1d, "Z"),
    key(0x1b, "X"),
    key(0x06, "C"),
    key(0x19, "V"),
    key(0x05, "B"),
    key(0x11, "N"),
    key(0x10, "M"),
    key(0x36, ","),
    key(0x37, "."),
    key(0x38, "/"),
    key(0xe5, "shift", 3),
  ],
  [
    key(0xe0, "control"),
    key(0xe2, "option"),
    key(0xe3, "command"),
    key(0x2c, "space", 5),
    key(0xe7, "command"),
    key(0xe6, "option"),
    key(0xe4, "control"),
    key(0x50, "←"),
    key(0x51, "↓"),
    key(0x52, "↑"),
    key(0x4f, "→"),
  ],
];

const numpadKeys: KeyDefinition[] = [
  key(0x53, "clear"),
  key(0x54, "/"),
  key(0x55, "*"),
  key(0x56, "-"),
  key(0x5f, "7"),
  key(0x60, "8"),
  key(0x61, "9"),
  key(0x57, "+", 1, 2),
  key(0x5c, "4"),
  key(0x5d, "5"),
  key(0x5e, "6"),
  key(0x59, "1"),
  key(0x5a, "2"),
  key(0x5b, "3"),
  key(0x58, "enter", 1, 2),
  key(0x62, "0", 2),
  key(0x63, "."),
];

export const emptyKeyboardMonitor: KeyboardMonitorState = {
  sequence: 0,
  pressed: [],
  recent: [],
};

export default function KeyboardMonitor({ monitor, caption }: KeyboardMonitorProps) {
  const [layout, setLayout] = useState<KeyboardLayoutKind>("compact");
  const pressed = useMemo(() => new Set(monitor.pressed), [monitor.pressed]);
  const labels = useMemo(() => keyLabels(mainRows, numpadKeys), []);

  return (
    <details className="keyboard-section">
      <summary className="keyboard-summary">
        <span className="keyboard-summary-title">键盘交互</span>
        <span className="keyboard-caption">
          {caption} · 当前按下 {monitor.pressed.length} 个键
        </span>
      </summary>

      <div className="keyboard-toolbar" aria-label="键盘布局">
        <span>布局</span>
        <div className="keyboard-layout-options">
          <button
            type="button"
            className={layout === "compact" ? "is-active" : ""}
            aria-pressed={layout === "compact"}
            onClick={() => setLayout("compact")}
          >
            紧凑键盘
          </button>
          <button
            type="button"
            className={layout === "full" ? "is-active" : ""}
            aria-pressed={layout === "full"}
            onClick={() => setLayout("full")}
          >
            全尺寸键盘
          </button>
        </div>
      </div>

      <div className="keyboard-scroll" aria-label="Mac 实时键盘状态">
        <div className={`keyboard-layout keyboard-layout--${layout}`}>
          <div className="keyboard-main">
            {mainRows.map((row, rowIndex) => (
              <div className="keyboard-row" key={rowIndex}>
                {row.map((definition, keyIndex) =>
                  renderKey(definition, pressed, `main-${rowIndex}-${keyIndex}`),
                )}
              </div>
            ))}
          </div>
          {layout === "full" ? (
            <div className="keyboard-numpad" aria-label="数字键盘">
              {numpadKeys.map((definition, index) =>
                renderKey(definition, pressed, `numpad-${index}`),
              )}
            </div>
          ) : null}
        </div>
      </div>

      <div className="keyboard-history">
        <span className="keyboard-history-label">最近事件</span>
        {monitor.recent.length === 0 ? (
          <span className="keyboard-history-empty">暂无按键</span>
        ) : (
          <ol>
            {monitor.recent.slice(0, 8).map((transition) => (
              <li key={transition.sequence}>
                <kbd>{labels.get(transition.usage) ?? hidLabel(transition.usage)}</kbd>
                <span className={`key-action key-action--${transition.action}`}>
                  {transition.action === "down" ? "按下" : "抬起"}
                </span>
              </li>
            ))}
          </ol>
        )}
      </div>
    </details>
  );
}

function renderKey(
  definition: KeyDefinition,
  pressed: ReadonlySet<number>,
  id: string,
) {
  const style = {
    "--key-span": definition.span ?? 1,
    "--key-row-span": definition.rowSpan ?? 1,
  } as CSSProperties;
  if (definition.spacer) {
    return <span className="keycap-spacer" style={style} key={id} aria-hidden="true" />;
  }
  const isPressed = pressed.has(definition.usage ?? -1);
  return (
    <kbd
      className={`keycap${isPressed ? " keycap--pressed" : ""}`}
      style={style}
      key={id}
      data-usage={definition.usage}
      data-pressed={isPressed ? "true" : "false"}
    >
      {definition.label}
    </kbd>
  );
}

function key(usage: number, label: string, span = 1, rowSpan = 1): KeyDefinition {
  return { usage, label, span, rowSpan };
}

function spacer(span = 1): KeyDefinition {
  return { span, spacer: true };
}

function keyLabels(rows: KeyDefinition[][], numpad: KeyDefinition[]): Map<number, string> {
  const labels = new Map<number, string>();
  for (const definition of [...rows.flat(), ...numpad]) {
    if (definition.usage !== undefined && definition.label !== undefined) {
      labels.set(definition.usage, definition.label);
    }
  }
  return labels;
}

function hidLabel(usage: number): string {
  return `HID 0x${usage.toString(16).padStart(2, "0")}`;
}
