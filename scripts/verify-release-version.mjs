import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const requestedVersion = process.argv[2];

if (!requestedVersion || !/^\d+\.\d+\.\d+$/.test(requestedVersion)) {
  console.error("Usage: node scripts/verify-release-version.mjs <major.minor.patch>");
  process.exit(1);
}

const errors = [];

async function readText(path) {
  return (await readFile(resolve(projectRoot, path), "utf8")).replaceAll("\r\n", "\n");
}

async function readJson(path) {
  return JSON.parse(await readText(path));
}

function expectVersion(label, actual) {
  if (actual !== requestedVersion) {
    errors.push(`${label}: expected ${requestedVersion}, found ${actual ?? "missing"}`);
  }
}

const jsonChecks = [
  ["package.json", "version"],
  ["packages/ui-contracts/package.json", "version"],
  ["apps/windows/ui/package.json", "version"],
  ["apps/windows/ui/package.json", "dependencies.@kvmflow/ui-contracts"],
  ["apps/macos/ui/package.json", "version"],
  ["apps/macos/ui/package.json", "dependencies.@kvmflow/ui-contracts"],
  ["apps/windows/tauri.conf.json", "version"],
  ["apps/macos/tauri.conf.json", "version"],
];

for (const [path, propertyPath] of jsonChecks) {
  const json = await readJson(path);
  const actual = propertyPath.split(".").reduce((value, key) => value?.[key], json);
  expectVersion(`${path}#${propertyPath}`, actual);
}

const packageLock = await readJson("package-lock.json");
expectVersion("package-lock.json#version", packageLock.version);
for (const packagePath of ["", "packages/ui-contracts", "apps/windows/ui", "apps/macos/ui"]) {
  expectVersion(`package-lock.json#packages.${packagePath || "<root>"}.version`, packageLock.packages?.[packagePath]?.version);
}
for (const packagePath of ["apps/windows/ui", "apps/macos/ui"]) {
  expectVersion(
    `package-lock.json#packages.${packagePath}.dependencies.@kvmflow/ui-contracts`,
    packageLock.packages?.[packagePath]?.dependencies?.["@kvmflow/ui-contracts"],
  );
}

const cargoToml = await readText("Cargo.toml");
const workspaceVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
expectVersion("Cargo.toml#workspace.package.version", workspaceVersion);

const localCrates = [
  "kvmflow-diagnostics",
  "kvmflow-domain",
  "kvmflow-engine",
  "kvmflow-macos-app",
  "kvmflow-platform-api",
  "kvmflow-platform-macos",
  "kvmflow-platform-windows",
  "kvmflow-protocol",
  "kvmflow-transport",
  "kvmflow-windows-app",
];
const cargoLock = await readText("Cargo.lock");
for (const crate of localCrates) {
  const escapedCrate = crate.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const packageBlock = cargoLock.match(
    new RegExp(`\\[\\[package\\]\\]\\nname = "${escapedCrate}"\\nversion = "([^"]+)"`),
  );
  expectVersion(`Cargo.lock#${crate}`, packageBlock?.[1]);
}

const changelog = await readText("CHANGELOG.md");
if (!changelog.includes(`## [${requestedVersion}]`)) {
  errors.push(`CHANGELOG.md: missing section ## [${requestedVersion}]`);
}

if (errors.length > 0) {
  console.error("Release version consistency check failed:");
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exit(1);
}

console.log(`Release version ${requestedVersion} is consistent.`);
