import { access, copyFile, cp, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  verifyCleanPackageLayout,
  verifyPackageLayout,
} from "./verify-package-layout.mjs";

const PLATFORM_APPS = {
  macos: "kvmFlow.app",
  windows: "kvmFlow.exe",
};

export async function exportCleanPackage(sourceRoot, destinationRoot, platform) {
  const appName = PLATFORM_APPS[platform];
  if (!appName) {
    throw new Error(`unsupported package platform: ${platform}`);
  }

  const source = verifyPackageLayout(sourceRoot, platform);
  const destination = path.resolve(destinationRoot);
  if (destination === source || destination.startsWith(`${source}${path.sep}`)) {
    throw new Error("release destination must be outside the runtime package");
  }
  let destinationExists = true;
  try {
    await access(destination);
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") {
      destinationExists = false;
    } else {
      throw error;
    }
  }
  if (destinationExists) {
    throw new Error(`release destination already exists: ${destination}`);
  }

  await mkdir(path.dirname(destination), { recursive: true });
  await mkdir(destination);
  try {
    await cp(path.join(source, appName), path.join(destination, appName), {
      recursive: true,
      errorOnExist: true,
      force: false,
    });
    await Promise.all(
      ["data", "logs", "diagnostics"].map((directory) =>
        mkdir(path.join(destination, directory)),
      ),
    );

    const sourceBuildId = path.join(source, "build-id.txt");
    try {
      await copyFile(sourceBuildId, path.join(destination, "build-id.txt"));
    } catch (error) {
      if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) {
        throw error;
      }
    }

    verifyCleanPackageLayout(destination, platform);
    return destination;
  } catch (error) {
    await rm(destination, { recursive: true, force: true });
    throw error;
  }
}

const entryPath = process.argv[1];
if (entryPath && import.meta.url === pathToFileURL(entryPath).href) {
  try {
    const [sourceRoot, destinationRoot, platform] = process.argv.slice(2);
    if (!sourceRoot || !destinationRoot || !platform) {
      throw new Error(
        "usage: node scripts/export-clean-package.mjs <source-root> <destination-root> <windows|macos>",
      );
    }
    console.log(await exportCleanPackage(sourceRoot, destinationRoot, platform));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
