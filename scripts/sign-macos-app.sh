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

identity="${KVMFLOW_MACOS_SIGNING_IDENTITY:-}"
keychain_path="${KVMFLOW_MACOS_SIGNING_KEYCHAIN:-}"

if [[ -z "$identity" ]]; then
  identity="${KVMFLOW_LOCAL_SIGNING_IDENTITY:-kvmFlow Local Development}"
  state_root="${KVMFLOW_LOCAL_SIGNING_ROOT:-$HOME/Library/Application Support/kvmFlow Development Signing}"
  keychain_path="$state_root/signing.keychain-db"
  password_path="$state_root/keychain-password"
  if [[ ! -f "$keychain_path" || ! -f "$password_path" ]]; then
    echo "stable macOS signing identity is unavailable" >&2
    echo "run scripts/setup-local-macos-signing.sh once, or set KVMFLOW_MACOS_SIGNING_IDENTITY" >&2
    exit 1
  fi
  keychain_password="$(<"$password_path")"
  security unlock-keychain -p "$keychain_password" "$keychain_path"
fi

if [[ "$identity" == "-" ]]; then
  echo "ad hoc signing is not allowed because it resets Keychain and Accessibility authorization" >&2
  exit 1
fi

original_keychains=()
search_list_changed=false
restore_keychain_search_list() {
  exit_code=$?
  trap - EXIT
  if [[ "$search_list_changed" == true ]]; then
    if ! security list-keychains -d user -s "${original_keychains[@]}"; then
      echo "failed to restore the user keychain search list" >&2
      exit_code=1
    fi
  fi
  exit "$exit_code"
}
trap restore_keychain_search_list EXIT

if [[ -n "$keychain_path" ]]; then
  keychain_is_listed=false
  while IFS= read -r line; do
    listed_keychain="${line#"${line%%[![:space:]]*}"}"
    listed_keychain="${listed_keychain#\"}"
    listed_keychain="${listed_keychain%\"}"
    [[ -n "$listed_keychain" ]] || continue
    original_keychains+=("$listed_keychain")
    if [[ "$listed_keychain" == "$keychain_path" ]]; then
      keychain_is_listed=true
    fi
  done < <(security list-keychains -d user)
  if [[ "$keychain_is_listed" == false ]]; then
    security list-keychains -d user -s "$keychain_path" "${original_keychains[@]}"
    search_list_changed=true
  fi
fi

signing_arguments=(--force --deep --sign "$identity" --options runtime)
if [[ -n "$keychain_path" ]]; then
  signing_arguments+=(--keychain "$keychain_path")
fi

codesign "${signing_arguments[@]}" "$app_path"
codesign --verify --deep --strict "$app_path"

designated_requirement="$(codesign -d -r- "$app_path" 2>&1)"
if [[ "$designated_requirement" == *"designated => cdhash"* ]]; then
  echo "macOS signature still uses a version-specific cdhash requirement" >&2
  exit 1
fi
printf '%s\n' "$designated_requirement"
