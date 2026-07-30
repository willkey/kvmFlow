import { existsSync, readdirSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const PLATFORM_APPS = {
  macos: "kvmFlow.app",
  windows: "kvmFlow.exe",
};
const RUNTIME_DIRECTORIES = {
  macos: [],
  windows: ["data", "logs", "diagnostics"],
};

export function verifyPackageLayout(root, platform) {
  const appName = PLATFORM_APPS[platform];
  if (!appName) {
    throw new Error(`unsupported package platform: ${platform}`);
  }

  const absoluteRoot = path.resolve(root);
  const runtimeDirectories = RUNTIME_DIRECTORIES[platform];
  const requiredPaths = [appName, ...runtimeDirectories];
  for (const relativePath of requiredPaths) {
    const requiredPath = path.join(absoluteRoot, relativePath);
    if (!existsSync(requiredPath)) {
      throw new Error(`missing required package path: ${requiredPath}`);
    }
  }

  return absoluteRoot;
}

export function verifyCleanPackageLayout(root, platform) {
  const absoluteRoot = verifyPackageLayout(root, platform);
  const allowedEntries = new Set([
    PLATFORM_APPS[platform],
    ...RUNTIME_DIRECTORIES[platform],
    ...(platform === "windows" ? ["build-id.txt"] : []),
  ]);
  for (const entry of readdirSync(absoluteRoot)) {
    if (!allowedEntries.has(entry)) {
      throw new Error(
        `unexpected path in release package: ${path.join(absoluteRoot, entry)}`,
      );
    }
  }
  for (const relativePath of RUNTIME_DIRECTORIES[platform]) {
    const directory = path.join(absoluteRoot, relativePath);
    if (readdirSync(directory).length !== 0) {
      throw new Error(`release package directory must be empty: ${directory}`);
    }
  }
  return absoluteRoot;
}

const entryPath = process.argv[1];
if (entryPath && import.meta.url === pathToFileURL(entryPath).href) {
  try {
    const [root, platform, mode] = process.argv.slice(2);
    if (!root || !platform) {
      throw new Error(
        "usage: node scripts/verify-package-layout.mjs <root> <windows|macos> [--clean]",
      );
    }
    if (mode === "--clean") {
      verifyCleanPackageLayout(root, platform);
    } else if (mode) {
      throw new Error(`unsupported package verification mode: ${mode}`);
    } else {
      verifyPackageLayout(root, platform);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
