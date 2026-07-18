import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  verifyCleanPackageLayout,
  verifyPackageLayout,
} from "./verify-package-layout.mjs";

async function createPackageRoot(entries) {
  const root = await mkdtemp(path.join(os.tmpdir(), "kvmflow-package-layout-"));
  await Promise.all(entries.map((entry) => mkdir(path.join(root, entry))));
  return root;
}

test("accepts a complete macOS package root", async (context) => {
  const root = await createPackageRoot([
    "kvmFlow.app",
    "data",
    "logs",
    "diagnostics",
  ]);
  context.after(() => rm(root, { recursive: true, force: true }));

  assert.equal(verifyPackageLayout(root, "macos"), path.resolve(root));
});

test("rejects a Windows package root without the executable", async (context) => {
  const root = await createPackageRoot(["data", "logs", "diagnostics"]);
  context.after(() => rm(root, { recursive: true, force: true }));

  assert.throws(
    () => verifyPackageLayout(root, "windows"),
    new Error(`missing required package path: ${path.join(root, "kvmFlow.exe")}`),
  );
});

test("rejects a package root without diagnostics", async (context) => {
  const root = await createPackageRoot(["kvmFlow.app", "data", "logs"]);
  context.after(() => rm(root, { recursive: true, force: true }));

  assert.throws(
    () => verifyPackageLayout(root, "macos"),
    new Error(`missing required package path: ${path.join(root, "diagnostics")}`),
  );
});

test("clean verification rejects runtime identity data", async (context) => {
  const root = await createPackageRoot([
    "kvmFlow.app",
    "data",
    "logs",
    "diagnostics",
  ]);
  context.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(path.join(root, "data", "device-identity.json"), "secret");

  assert.throws(
    () => verifyCleanPackageLayout(root, "macos"),
    new Error(
      `release package directory must be empty: ${path.join(root, "data")}`,
    ),
  );
});

test("clean verification rejects unexpected top-level files", async (context) => {
  const root = await createPackageRoot([
    "kvmFlow.app",
    "data",
    "logs",
    "diagnostics",
  ]);
  context.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(path.join(root, "private-key.txt"), "secret");

  assert.throws(
    () => verifyCleanPackageLayout(root, "macos"),
    new Error(
      `unexpected path in release package: ${path.join(root, "private-key.txt")}`,
    ),
  );
});
