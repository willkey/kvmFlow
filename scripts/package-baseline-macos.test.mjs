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
  assert.match(script, /npm run tauri:macos:build -- --bundles app/);
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
  assert.match(signer, /KVMFLOW_RCODESIGN_VERSION/);
  assert.match(signer, /apple-codesign --version/);
  assert.match(signer, /designated => cdhash/);
  assert.match(signer, /signing-private-key\.pem/);
  assert.match(signer, /rcodesign sign/);
  assert.match(signer, /--pem-file/);
  assert.match(signer, /--timestamp-url none/);
  assert.match(signer, /rcodesign verify/);
  assert.doesNotMatch(signer, /security/);
  assert.doesNotMatch(signer, /keychain/i);
  assert.match(setup, /signing-private-key\.pem/);
  assert.match(setup, /chmod 600 "\$signing_private_key_path"/);
  assert.match(setup, /extendedKeyUsage=critical,codeSigning/);
  assert.doesNotMatch(setup, /security/);
  assert.doesNotMatch(setup, /keychain/i);
  assert.match(setup, /--remove/);
});
