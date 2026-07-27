import fixture from "../../../fixtures/app-snapshot.json";
import { describe, expect, it } from "vitest";

import {
  groupKeyboardInteractions,
  occupiedDisplayEdges,
  parseAppSnapshot,
  parseDisplayTargets,
  parseKeyboardMonitorState,
  type AppSnapshot,
} from "./index.js";

describe("occupiedDisplayEdges", () => {
  const displays = [
    {
      id: "primary",
      primary: true,
      left: 0,
      top: 0,
      width: 1920,
      height: 1080,
    },
    {
      id: "secondary",
      primary: false,
      left: 1920,
      top: -180,
      width: 2560,
      height: 1440,
    },
  ];

  it("blocks each side of a shared Windows display boundary", () => {
    expect(occupiedDisplayEdges(displays, "primary")).toEqual(["right"]);
    expect(occupiedDisplayEdges(displays, "secondary")).toEqual(["left"]);
  });

  it("leaves a direction available when displays only touch at a corner", () => {
    expect(
      occupiedDisplayEdges(
        [
          displays[0],
          {
            id: "corner",
            primary: false,
            left: 1920,
            top: 1080,
            width: 800,
            height: 600,
          },
        ],
        "primary",
      ),
    ).toEqual([]);
  });
});

describe("parseDisplayTargets", () => {
  it("accepts negative desktop coordinates and rejects invalid dimensions", () => {
    expect(
      parseDisplayTargets([
        {
          id: "\\\\.\\DISPLAY2",
          primary: false,
          left: -2560,
          top: -360,
          width: 2560,
          height: 1440,
        },
      ]),
    ).toEqual([
      {
        id: "\\\\.\\DISPLAY2",
        primary: false,
        left: -2560,
        top: -360,
        width: 2560,
        height: 1440,
      },
    ]);
    expect(() =>
      parseDisplayTargets([
        {
          id: "display",
          primary: true,
          left: 0,
          top: 0,
          width: -1,
          height: 1080,
        },
      ]),
    ).toThrow(TypeError);
  });
});

function cloneFixture(): Record<string, unknown> {
  return structuredClone(fixture) as Record<string, unknown>;
}

function setValue(
  value: Record<string, unknown>,
  path: readonly string[],
  replacement: unknown,
): void {
  let current = value;

  for (const segment of path.slice(0, -1)) {
    current = current[segment] as Record<string, unknown>;
  }

  current[path[path.length - 1]!] = replacement;
}

function populatedSnapshot(): Record<string, unknown> {
  const input = cloneFixture();

  input.latestResource = {
    observedAtUnixMs: 1,
    uptimeSeconds: 2,
    windowState: "open",
    processCount: 3,
    residentMemoryBytes: 4,
    cpuPercent: 5,
    cpuValid: false,
  };

  return input;
}

describe("parseAppSnapshot", () => {
  it("parses every field from the stable Rust fixture", () => {
    const snapshot: AppSnapshot = parseAppSnapshot(fixture);

    expect(snapshot).toEqual(fixture);
    expect(snapshot.role).toBe("windowsController");
    expect(snapshot.phase).toBe("running");
    expect(snapshot.build.buildId).toBe("fixture-build");
    expect(snapshot.validation.backgroundMemory.target).toBe(52_428_800);
    expect(snapshot.validation.visibleMemory.target).toBe(188_743_680);
    expect(snapshot.validation.backgroundCpu.target).toBe(0.5);
  });

  it("accepts nullable fields and populated nested resource and notice values", () => {
    const input = cloneFixture();

    input.diagnosticNotice = {
      code: "storageFallback",
      message: "Using an emergency directory",
      path: "/tmp/kvmflow",
    };
    input.latestResource = {
      observedAtUnixMs: 1_722_345_678_901,
      uptimeSeconds: 42,
      windowState: "closed",
      processCount: 3,
      residentMemoryBytes: 10_485_760,
      cpuPercent: 0.25,
      cpuValid: true,
    };
    const validation = input.validation as Record<string, unknown>;
    const backgroundMemory = validation.backgroundMemory as Record<string, unknown>;
    backgroundMemory.current = 4_194_304;

    const snapshot = parseAppSnapshot(input);

    expect(snapshot.diagnosticNotice).toEqual(input.diagnosticNotice);
    expect(snapshot.latestResource).toEqual(input.latestResource);
    expect(snapshot.validation.backgroundMemory.current).toBe(4_194_304);
  });

  it("tolerates unknown fields at every object level without retaining or mutating them", () => {
    const input = cloneFixture();
    const expectedInput = structuredClone(input);

    input.extra = "future root field";
    (input.build as Record<string, unknown>).extra = "future build field";
    (input.validation as Record<string, unknown>).extra = "future validation field";
    (
      (input.validation as Record<string, unknown>).backgroundMemory as Record<
        string,
        unknown
      >
    ).extra = "future metric field";
    input.diagnosticNotice = {
      code: "storageFallback",
      message: "Fallback enabled",
      path: null,
      extra: "future notice field",
    };
    input.latestResource = {
      observedAtUnixMs: 1,
      uptimeSeconds: 2,
      windowState: "open",
      processCount: 3,
      residentMemoryBytes: 4,
      cpuPercent: 5,
      cpuValid: false,
      extra: "future resource field",
    };

    const snapshot = parseAppSnapshot(input);

    expect(input).toEqual({
      ...expectedInput,
      extra: "future root field",
      build: { ...fixture.build, extra: "future build field" },
      validation: {
        ...fixture.validation,
        extra: "future validation field",
        backgroundMemory: {
          ...fixture.validation.backgroundMemory,
          extra: "future metric field",
        },
      },
      diagnosticNotice: {
        code: "storageFallback",
        message: "Fallback enabled",
        path: null,
        extra: "future notice field",
      },
      latestResource: {
        observedAtUnixMs: 1,
        uptimeSeconds: 2,
        windowState: "open",
        processCount: 3,
        residentMemoryBytes: 4,
        cpuPercent: 5,
        cpuValid: false,
        extra: "future resource field",
      },
    });
    expect(snapshot).toEqual({
      ...fixture,
      diagnosticNotice: {
        code: "storageFallback",
        message: "Fallback enabled",
        path: null,
      },
      latestResource: {
        observedAtUnixMs: 1,
        uptimeSeconds: 2,
        windowState: "open",
        processCount: 3,
        residentMemoryBytes: 4,
        cpuPercent: 5,
        cpuValid: false,
      },
    });
  });

  it.each([
    [["schemaVersion"], 7],
    [["role"], "linuxEndpoint"],
    [["phase"], "paused"],
    [["windowState"], "minimized"],
    [["validation", "backgroundMemory", "status"], "unknown"],
    [["diagnosticNotice", "code"], "unknown"],
    [["latestResource", "windowState"], "minimized"],
  ] as const)("rejects an unsupported enum at %s", (path, replacement) => {
    const input = cloneFixture();

    if (path[0] === "diagnosticNotice") {
      input.diagnosticNotice = {
        code: "storageFallback",
        message: "Fallback enabled",
        path: null,
      };
    }
    if (path[0] === "latestResource") {
      input.latestResource = {
        observedAtUnixMs: 1,
        uptimeSeconds: 2,
        windowState: "open",
        processCount: 3,
        residentMemoryBytes: 4,
        cpuPercent: 5,
        cpuValid: false,
      };
    }
    setValue(input, path, replacement);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it.each([
    [["build", "buildId"]],
    [["validation", "backgroundCpu", "target"]],
    [["latestResource", "cpuValid"]],
  ] as const)("rejects missing required field at %s", (path) => {
    const input = cloneFixture();

    if (path[0] === "latestResource") {
      input.latestResource = {
        observedAtUnixMs: 1,
        uptimeSeconds: 2,
        windowState: "open",
        processCount: 3,
        residentMemoryBytes: 4,
        cpuPercent: 5,
        cpuValid: false,
      };
    }
    const parentPath = path.slice(0, -1);
    let parent = input;
    for (const segment of parentPath) {
      parent = parent[segment] as Record<string, unknown>;
    }
    delete parent[path[path.length - 1]!];

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it.each([
    [["diagnosticNotice", "message"], 1],
    [["diagnosticNotice", "path"], 1],
    [["latestResource", "cpuValid"], "false"],
  ] as const)("rejects invalid notice and boolean fields at %s", (path, replacement) => {
    const input = cloneFixture();

    if (path[0] === "diagnosticNotice") {
      input.diagnosticNotice = {
        code: "storageFallback",
        message: "Fallback enabled",
        path: null,
      };
    }
    if (path[0] === "latestResource") {
      input.latestResource = {
        observedAtUnixMs: 1,
        uptimeSeconds: 2,
        windowState: "open",
        processCount: 3,
        residentMemoryBytes: 4,
        cpuPercent: 5,
        cpuValid: false,
      };
    }
    setValue(input, path, replacement);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it.each([
    [["latestResource", "observedAtUnixMs"]],
    [["latestResource", "uptimeSeconds"]],
    [["latestResource", "processCount"]],
    [["latestResource", "residentMemoryBytes"]],
    [["latestResource", "cpuPercent"]],
    [["validation", "backgroundMemory", "current"]],
    [["validation", "visibleMemory", "target"]],
    [["validation", "backgroundCpu", "target"]],
    [["validation", "continuousUptimeSeconds"]],
  ] as const)("rejects non-finite numeric field at %s", (path) => {
    const input = cloneFixture();

    if (path[0] === "latestResource") {
      input.latestResource = {
        observedAtUnixMs: 1,
        uptimeSeconds: 2,
        windowState: "open",
        processCount: 3,
        residentMemoryBytes: 4,
        cpuPercent: 5,
        cpuValid: false,
      };
    }
    setValue(input, path, Number.NaN);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it.each([
    [["latestResource", "observedAtUnixMs"]],
    [["latestResource", "uptimeSeconds"]],
    [["latestResource", "processCount"]],
    [["latestResource", "residentMemoryBytes"]],
    [["validation", "backgroundMemory", "current"]],
    [["validation", "backgroundMemory", "target"]],
    [["validation", "visibleMemory", "current"]],
    [["validation", "visibleMemory", "target"]],
    [["validation", "continuousUptimeSeconds"]],
  ] as const)("rejects negative timestamp, uptime, memory, and count field at %s", (path) => {
    const input = cloneFixture();

    if (path[0] === "latestResource") {
      input.latestResource = {
        observedAtUnixMs: 1,
        uptimeSeconds: 2,
        windowState: "open",
        processCount: 3,
        residentMemoryBytes: 4,
        cpuPercent: 5,
        cpuValid: false,
      };
    }
    setValue(input, path, -1);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it.each([
    [["latestResource", "observedAtUnixMs"]],
    [["latestResource", "uptimeSeconds"]],
    [["latestResource", "residentMemoryBytes"]],
    [["validation", "continuousUptimeSeconds"]],
  ] as const)("rejects fractional u64 field at %s", (path) => {
    const input = populatedSnapshot();
    setValue(input, path, 1.5);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it.each([
    [["latestResource", "observedAtUnixMs"]],
    [["latestResource", "uptimeSeconds"]],
    [["latestResource", "residentMemoryBytes"]],
    [["validation", "continuousUptimeSeconds"]],
  ] as const)("rejects an unsafe u64 field at %s", (path) => {
    const input = populatedSnapshot();
    setValue(input, path, Number.MAX_SAFE_INTEGER + 1);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it("rejects a fractional process count", () => {
    const input = populatedSnapshot();
    setValue(input, ["latestResource", "processCount"], 1.5);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it("rejects a process count above u32::MAX", () => {
    const input = populatedSnapshot();
    setValue(input, ["latestResource", "processCount"], 4_294_967_296);

    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });

  it("accepts the maximum safe u64 and u32 values", () => {
    const input = populatedSnapshot();
    setValue(input, ["latestResource", "observedAtUnixMs"], Number.MAX_SAFE_INTEGER);
    setValue(input, ["latestResource", "uptimeSeconds"], Number.MAX_SAFE_INTEGER);
    setValue(input, ["latestResource", "processCount"], 4_294_967_295);
    setValue(input, ["latestResource", "residentMemoryBytes"], Number.MAX_SAFE_INTEGER);
    setValue(input, ["validation", "continuousUptimeSeconds"], Number.MAX_SAFE_INTEGER);

    const snapshot = parseAppSnapshot(input);

    expect(snapshot.latestResource).toMatchObject({
      observedAtUnixMs: Number.MAX_SAFE_INTEGER,
      uptimeSeconds: Number.MAX_SAFE_INTEGER,
      processCount: 4_294_967_295,
      residentMemoryBytes: Number.MAX_SAFE_INTEGER,
    });
    expect(snapshot.validation.continuousUptimeSeconds).toBe(Number.MAX_SAFE_INTEGER);
  });

  it.each([null, [], "snapshot", 1])("rejects a non-object snapshot", (input) => {
    expect(() => parseAppSnapshot(input)).toThrow(TypeError);
  });
});

describe("parseKeyboardMonitorState", () => {
  it("parses pressed usages and recent keyboard transitions", () => {
    expect(
      parseKeyboardMonitorState({
        sequence: 3,
        pressed: [0xe3, 0x52],
        recent: [
          { sequence: 3, usage: 0x52, action: "down" },
          { sequence: 2, usage: 0xe3, action: "down" },
        ],
      }),
    ).toEqual({
      sequence: 3,
      pressed: [0xe3, 0x52],
      recent: [
        { sequence: 3, usage: 0x52, action: "down" },
        { sequence: 2, usage: 0xe3, action: "down" },
      ],
    });
  });

  it.each([
    { sequence: -1, pressed: [], recent: [] },
    { sequence: 1, pressed: "not-an-array", recent: [] },
    { sequence: 1, pressed: [65_536], recent: [] },
    { sequence: 1, pressed: [], recent: "not-an-array" },
    {
      sequence: 1,
      pressed: [],
      recent: [{ sequence: 1, usage: 4, action: "repeat" }],
    },
    {
      sequence: 13,
      pressed: [],
      recent: Array.from({ length: 13 }, (_, index) => ({
        sequence: index + 1,
        usage: 4,
        action: "down",
      })),
    },
  ])("rejects malformed monitor payload %#", (input) => {
    expect(() => parseKeyboardMonitorState(input)).toThrow(TypeError);
  });
});

describe("groupKeyboardInteractions", () => {
  it("groups arbitrary modifiers and a main key into one completed interaction", () => {
    expect(
      groupKeyboardInteractions({
        sequence: 6,
        pressed: [],
        recent: [
          { sequence: 6, usage: 0xe3, action: "up" },
          { sequence: 5, usage: 0xe1, action: "up" },
          { sequence: 4, usage: 0x04, action: "up" },
          { sequence: 3, usage: 0x04, action: "down" },
          { sequence: 2, usage: 0xe1, action: "down" },
          { sequence: 1, usage: 0xe3, action: "down" },
        ],
      }),
    ).toEqual([
      {
        sequence: 3,
        usages: [0xe3, 0xe1, 0x04],
        combination: true,
        completed: true,
      },
    ]);
  });

  it("keeps one held modifier across multiple combinations and ignores repeats", () => {
    expect(
      groupKeyboardInteractions({
        sequence: 7,
        pressed: [],
        recent: [
          { sequence: 7, usage: 0xe3, action: "up" },
          { sequence: 6, usage: 0x19, action: "up" },
          { sequence: 5, usage: 0x19, action: "down" },
          { sequence: 4, usage: 0x06, action: "up" },
          { sequence: 3, usage: 0x06, action: "down" },
          { sequence: 2, usage: 0x06, action: "down" },
          { sequence: 1, usage: 0xe3, action: "down" },
        ],
      }),
    ).toEqual([
      {
        sequence: 5,
        usages: [0xe3, 0x19],
        combination: true,
        completed: true,
      },
      {
        sequence: 2,
        usages: [0xe3, 0x06],
        combination: true,
        completed: true,
      },
    ]);
  });

  it("shows a standalone lock key as one interaction", () => {
    expect(
      groupKeyboardInteractions({
        sequence: 2,
        pressed: [],
        recent: [
          { sequence: 2, usage: 0x39, action: "up" },
          { sequence: 1, usage: 0x39, action: "down" },
        ],
      }),
    ).toEqual([
      {
        sequence: 1,
        usages: [0x39],
        combination: false,
        completed: true,
      },
    ]);
  });
});
