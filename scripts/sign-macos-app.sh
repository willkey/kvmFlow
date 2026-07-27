#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/sign-macos-app.sh <app-path>" >&2
  exit 1
fi

app_path="$1"
if [[ ! -d "$app_path" ]]; then
  echo "macOS application bundle does not exist: $app_path" >&2
  exit 1
fi

required_rcodesign_version="${KVMFLOW_RCODESIGN_VERSION:-0.29.0}"
if ! command -v rcodesign >/dev/null 2>&1; then
  echo "rcodesign is unavailable" >&2
  echo "install it with: cargo install apple-codesign --version $required_rcodesign_version --locked" >&2
  exit 1
fi
actual_rcodesign_version="$(rcodesign --version)"
if [[ "$actual_rcodesign_version" != "apple-codesign $required_rcodesign_version" ]]; then
  echo "rcodesign version mismatch: expected $required_rcodesign_version, found $actual_rcodesign_version" >&2
  exit 1
fi

state_root="${KVMFLOW_LOCAL_SIGNING_ROOT:-$HOME/Library/Application Support/kvmFlow Development Signing}"
signing_private_key_path="$state_root/signing-private-key.pem"
signing_certificate_path="$state_root/signing-certificate.pem"
if [[ ! -f "$signing_private_key_path" || ! -f "$signing_certificate_path" ]]; then
  echo "stable macOS signing files are unavailable" >&2
  echo "run scripts/setup-local-macos-signing.sh once" >&2
  exit 1
fi

rcodesign sign \
  --pem-file "$signing_private_key_path" \
  --pem-file "$signing_certificate_path" \
  --timestamp-url none \
  --code-signature-flags runtime \
  "$app_path"

main_executable="$app_path/Contents/MacOS/kvmFlow"
rcodesign verify "$main_executable"

designated_requirement="$(codesign -d -r- "$app_path" 2>&1)"
if [[ "$designated_requirement" == *"designated => cdhash"* \
  || "$designated_requirement" != *"certificate root"* ]]; then
  echo "macOS signature does not have a stable certificate requirement" >&2
  exit 1
fi
printf '%s\n' "$designated_requirement"
