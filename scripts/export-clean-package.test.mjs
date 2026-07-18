import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { exportCleanPackage } from "./export-clean-package.mjs";

test("clean macOS export excludes identity, config, logs, and diagnostics", async (context) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "kvmflow-clean-export-"));
  const source = path.join(root, "runtime");
  const destination = path.join(root, "release");
  context.after(() => rm(root, { recursive: true, force: true }));

  await mkdir(path.join(source, "kvmFlow.app"), { recursive: true });
  await mkdir(path.join(source, "data"));
  await mkdir(path.join(source, "logs"));
  await mkdir(path.join(source, "diagnostics"));
  await writeFile(path.join(source, "kvmFlow.app", "marker"), "app");
  await writeFile(path.join(source, "data", "device-identity.json"), "secret");
  await writeFile(path.join(source, "data", "config.json"), "private config");
  await writeFile(path.join(source, "logs", "kvmflow.jsonl"), "log");
  await writeFile(path.join(source, "diagnostics", "report.json"), "report");
  await writeFile(path.join(source, "build-id.txt"), "test-build\n");

  assert.equal(
    await exportCleanPackage(source, destination, "macos"),
    destination,
  );
  assert.equal(
    await readFile(path.join(destination, "kvmFlow.app", "marker"), "utf8"),
    "app",
  );
  assert.deepEqual(await readdir(path.join(destination, "data")), []);
  assert.deepEqual(await readdir(path.join(destination, "logs")), []);
  assert.deepEqual(await readdir(path.join(destination, "diagnostics")), []);
  assert.equal(
    await readFile(path.join(destination, "build-id.txt"), "utf8"),
    "test-build\n",
  );
});

test("clean export refuses to merge into an existing destination", async (context) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "kvmflow-clean-export-"));
  const source = path.join(root, "runtime");
  const destination = path.join(root, "release");
  context.after(() => rm(root, { recursive: true, force: true }));

  await mkdir(source);
  await writeFile(path.join(source, "kvmFlow.exe"), "exe");
  await Promise.all(
    ["data", "logs", "diagnostics"].map((directory) =>
      mkdir(path.join(source, directory)),
    ),
  );
  await mkdir(destination);
  await writeFile(path.join(destination, "keep.txt"), "keep");

  await assert.rejects(
    exportCleanPackage(source, destination, "windows"),
    new Error(`release destination already exists: ${destination}`),
  );
  assert.equal(
    await readFile(path.join(destination, "keep.txt"), "utf8"),
    "keep",
  );
});
