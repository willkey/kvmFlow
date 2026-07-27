#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "local macOS signing setup must run on macOS" >&2
  exit 1
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "required command is unavailable: openssl" >&2
  exit 1
fi

identity_name="${KVMFLOW_LOCAL_SIGNING_IDENTITY:-kvmFlow Local Development}"
state_root="${KVMFLOW_LOCAL_SIGNING_ROOT:-$HOME/Library/Application Support/kvmFlow Development Signing}"
signing_private_key_path="$state_root/signing-private-key.pem"
signing_certificate_path="$state_root/signing-certificate.pem"

if [[ "${1:-}" == "--remove" ]]; then
  rm -f "$signing_private_key_path" "$signing_certificate_path"
  rmdir "$state_root" >/dev/null 2>&1 || true
  printf 'removed local macOS signing files: %s\n' "$identity_name"
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "usage: scripts/setup-local-macos-signing.sh [--remove]" >&2
  exit 1
fi

identity_files_match() {
  certificate_modulus="$(openssl x509 -noout -modulus -in "$signing_certificate_path")"
  private_key_modulus="$(openssl rsa -noout -modulus -in "$signing_private_key_path" 2>/dev/null)"
  [[ -n "$certificate_modulus" && "$certificate_modulus" == "$private_key_modulus" ]]
}

if [[ -f "$signing_private_key_path" && -f "$signing_certificate_path" ]]; then
  if identity_files_match; then
    printf 'local macOS signing files are ready: %s\n' "$identity_name"
    printf 'private key: %s\n' "$signing_private_key_path"
    printf 'certificate: %s\n' "$signing_certificate_path"
    exit 0
  fi
  echo "local signing certificate does not match its private key" >&2
  echo "run scripts/setup-local-macos-signing.sh --remove before retrying" >&2
  exit 1
fi

if [[ -e "$signing_private_key_path" || -e "$signing_certificate_path" ]]; then
  echo "incomplete local signing state exists: $state_root" >&2
  echo "run scripts/setup-local-macos-signing.sh --remove before retrying" >&2
  exit 1
fi

umask 077
mkdir -p "$state_root"
temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/kvmflow-signing.XXXXXX")"

cleanup() {
  exit_code=$?
  rm -rf "$temporary_root"
  if [[ $exit_code -ne 0 ]]; then
    rm -f "$signing_private_key_path" "$signing_certificate_path"
  fi
  exit "$exit_code"
}
trap cleanup EXIT

openssl req -x509 -newkey rsa:3072 -sha256 -nodes \
  -keyout "$temporary_root/signing-private-key.pem" \
  -out "$temporary_root/signing-certificate.pem" \
  -days 3650 \
  -subj "/CN=$identity_name/O=kvmFlow" \
  -addext "basicConstraints=critical,CA:FALSE" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" \
  -addext "subjectKeyIdentifier=hash" \
  -addext "authorityKeyIdentifier=keyid" \
  >/dev/null 2>&1

cp "$temporary_root/signing-private-key.pem" "$signing_private_key_path"
cp "$temporary_root/signing-certificate.pem" "$signing_certificate_path"
chmod 600 "$signing_private_key_path"
chmod 644 "$signing_certificate_path"

if ! identity_files_match; then
  echo "generated local signing files do not match" >&2
  exit 1
fi

printf 'local macOS signing files are ready: %s\n' "$identity_name"
printf 'private key: %s\n' "$signing_private_key_path"
printf 'certificate: %s\n' "$signing_certificate_path"
