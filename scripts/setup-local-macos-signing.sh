#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "local macOS signing setup must run on macOS" >&2
  exit 1
fi

for command in openssl security; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is unavailable: $command" >&2
    exit 1
  fi
done

identity_name="${KVMFLOW_LOCAL_SIGNING_IDENTITY:-kvmFlow Local Development}"
state_root="${KVMFLOW_LOCAL_SIGNING_ROOT:-$HOME/Library/Application Support/kvmFlow Development Signing}"
keychain_path="$state_root/signing.keychain-db"
password_path="$state_root/keychain-password"
signing_certificate_path="$state_root/signing-certificate.pem"

if [[ "${1:-}" == "--remove" ]]; then
  if [[ -f "$signing_certificate_path" ]]; then
    security remove-trusted-cert "$signing_certificate_path"
  fi
  if [[ -f "$keychain_path" ]]; then
    security delete-keychain "$keychain_path"
  fi
  rm -f "$password_path" "$signing_certificate_path"
  rmdir "$state_root" >/dev/null 2>&1 || true
  printf 'removed local macOS signing identity: %s\n' "$identity_name"
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "usage: scripts/setup-local-macos-signing.sh [--remove]" >&2
  exit 1
fi

identity_is_available() {
  security find-identity -v -p codesigning "$keychain_path" 2>/dev/null \
    | grep -Fq "\"$identity_name\""
}

if [[ -f "$keychain_path" && -f "$password_path" && -f "$signing_certificate_path" ]]; then
  keychain_password="$(<"$password_path")"
  security unlock-keychain -p "$keychain_password" "$keychain_path"
  if identity_is_available; then
    printf 'local macOS signing identity is ready: %s\n' "$identity_name"
    printf 'keychain: %s\n' "$keychain_path"
    exit 0
  fi
  echo "local signing files exist, but the identity is not trusted for code signing" >&2
  echo "remove the incomplete directory before retrying: $state_root" >&2
  exit 1
fi

if [[ -e "$keychain_path" || -e "$password_path" || -e "$signing_certificate_path" ]]; then
  echo "incomplete local signing state exists: $state_root" >&2
  echo "remove the incomplete directory before retrying" >&2
  exit 1
fi

umask 077
mkdir -p "$state_root"
temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/kvmflow-signing.XXXXXX")"
created_state=true

cleanup() {
  exit_code=$?
  rm -rf "$temporary_root"
  if [[ $exit_code -ne 0 && "$created_state" == true ]]; then
    if [[ -f "$signing_certificate_path" ]]; then
      security remove-trusted-cert "$signing_certificate_path" >/dev/null 2>&1 || true
    fi
    security delete-keychain "$keychain_path" >/dev/null 2>&1 || true
    rm -f "$password_path" "$signing_certificate_path"
  fi
  exit "$exit_code"
}
trap cleanup EXIT

keychain_password="$(openssl rand -hex 32)"
printf '%s\n' "$keychain_password" > "$password_path"

security create-keychain -p "$keychain_password" "$keychain_path"
security unlock-keychain -p "$keychain_password" "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"

openssl req -x509 -newkey rsa:3072 -sha256 -nodes \
  -keyout "$temporary_root/identity.key" \
  -out "$temporary_root/identity.crt" \
  -days 3650 \
  -subj "/CN=$identity_name/O=kvmFlow" \
  -addext "basicConstraints=critical,CA:FALSE" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" \
  -addext "subjectKeyIdentifier=hash" \
  -addext "authorityKeyIdentifier=keyid" \
  >/dev/null 2>&1

openssl pkcs12 -export -legacy \
  -out "$temporary_root/identity.p12" \
  -inkey "$temporary_root/identity.key" \
  -in "$temporary_root/identity.crt" \
  -name "$identity_name" \
  -passout "pass:$keychain_password"

security import "$temporary_root/identity.p12" \
  -k "$keychain_path" \
  -P "$keychain_password" \
  -x \
  -T /usr/bin/codesign
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s \
  -k "$keychain_password" \
  "$keychain_path" \
  >/dev/null

cp "$temporary_root/identity.crt" "$signing_certificate_path"
chmod 600 "$password_path"
chmod 644 "$signing_certificate_path"
security add-trusted-cert \
  -r trustRoot \
  -p codeSign \
  -k "$keychain_path" \
  "$signing_certificate_path"

if ! identity_is_available; then
  echo "local signing identity was created but is not available to codesign" >&2
  exit 1
fi

created_state=false
printf 'local macOS signing identity is ready: %s\n' "$identity_name"
printf 'keychain: %s\n' "$keychain_path"
