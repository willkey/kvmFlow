import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import packageJson from "../package.json" with { type: "json" };

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scriptPath = resolve(projectRoot, "scripts/verify-release-version.mjs");

function run(version) {
  return spawnSync(process.execPath, [scriptPath, version], {
    cwd: projectRoot,
    encoding: "utf8",
  });
}

test("accepts the consistent project version", () => {
  const result = run(packageJson.version);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, new RegExp(`Release version ${packageJson.version.replaceAll(".", "\\.")} is consistent`));
});

test("rejects a mismatched release version", () => {
  const result = run("9.9.9");
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Release version consistency check failed/);
});

test("rejects a version with a v prefix", () => {
  const result = run(`v${packageJson.version}`);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Usage:/);
});
