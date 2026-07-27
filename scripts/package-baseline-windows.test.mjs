import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const script = await readFile(
  new URL("./package-baseline-windows.ps1", import.meta.url),
  "utf8",
);

test("Windows packaging tolerates SMB metadata without changing global Git trust", () => {
  assert.match(
    script,
    /git -c "safe\.directory=\*" -c "core\.filemode=false" status --porcelain --untracked-files=no/,
  );
  assert.doesNotMatch(script, /git config --global/);
});

test("Windows packaging builds committed HEAD in a local temporary workspace", () => {
  assert.match(script, /\$stagingRoot = Join-Path \$env:TEMP/);
  assert.match(script, /archive --format=zip/);
  assert.match(script, /Expand-Archive/);
  assert.match(
    script,
    /Push-Location \$buildRoot[\s\S]*npm ci[\s\S]*npm run tauri:windows:build/,
  );
  assert.match(
    script,
    /Copy-Item \(Join-Path \$buildRoot "target\/release\/kvmFlow\.exe"\)/,
  );
  assert.match(
    script,
    /Remove-Item -LiteralPath \$stagingRoot -Recurse -Force/,
  );
});

test("Windows packaging replaces the executable at a stable path and preserves runtime data", () => {
  assert.match(script, /\$packageRoot = Join-Path \$repoRoot "dist\/windows\/current"/);
  assert.doesNotMatch(script, /"dist\/windows" \$buildId/);
  assert.match(
    script,
    /New-Item -ItemType Directory -Path \(Join-Path \$packageRoot \$directory\) -Force/,
  );
  assert.match(script, /\.kvmFlow\.exe\.previous/);
  assert.match(script, /Move-Item -LiteralPath \$nextExecutable -Destination \$executablePath/);
  assert.match(script, /build-id\.txt/);
});
