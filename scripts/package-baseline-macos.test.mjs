import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const script = await readFile(
  new URL("./package-baseline-macos.sh", import.meta.url),
  "utf8",
);
const signer = await readFile(
  new URL("./sign-macos-app.sh", import.meta.url),
  "utf8",
);
const setup = await readFile(
  new URL("./setup-local-macos-signing.sh", import.meta.url),
  "utf8",
);

test("macOS packaging ignores untracked transfer files and filemode noise", () => {
  assert.match(script, /core\.filemode=false/);
  assert.match(script, /--untracked-files=no/);
  assert.match(script, /cargo tauri build --bundles app/);
  assert.match(script, /scripts\/sign-macos-app\.sh/);
  assert.doesNotMatch(script, /--sign -/);
});

test("macOS packaging replaces the app at a stable path and preserves runtime data", () => {
  assert.match(script, /package_root="dist\/macos\/current"/);
  assert.doesNotMatch(script, /package_root="dist\/macos\/\$build_id"/);
  assert.match(script, /mkdir -p "\$package_root\/data"/);
  assert.match(script, /previous_app="\$package_root\/\.kvmFlow\.app\.previous"/);
  assert.match(script, /! -d "\$package_root\/kvmFlow\.app" && -d "\$previous_app"/);
  assert.match(script, /mv "\$next_app" "\$package_root\/kvmFlow\.app"/);
  assert.match(script, /build-id\.txt/);
});

test("macOS packaging requires a stable signing identity", () => {
  assert.match(signer, /KVMFLOW_MACOS_SIGNING_IDENTITY/);
  assert.match(signer, /ad hoc signing is not allowed/);
  assert.match(signer, /designated => cdhash/);
  assert.match(signer, /trap restore_keychain_search_list EXIT/);
  assert.match(signer, /security list-keychains -d user -s/);
  assert.match(signer, /codesign --verify --deep --strict/);
  assert.match(setup, /extendedKeyUsage=critical,codeSigning/);
  assert.match(setup, /\n  -x \\\n/);
  assert.match(setup, /set-key-partition-list/);
  assert.match(setup, /-p codeSign/);
  assert.match(setup, /--remove/);
  assert.match(setup, /security remove-trusted-cert/);
});
