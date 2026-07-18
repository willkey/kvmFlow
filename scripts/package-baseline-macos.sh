#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ -n "$(git -c core.filemode=false status --porcelain --untracked-files=no)" ]]; then
  echo "refusing to package from a dirty Git worktree" >&2
  exit 1
fi

build_id="$(date -u +%Y%m%d-%H%M%S)-$(git rev-parse --short=12 HEAD)"
package_root="dist/macos/current"
next_app="$package_root/.kvmFlow.app.next-$build_id"
previous_app="$package_root/.kvmFlow.app.previous"

cleanup_staging_app() {
  rm -rf "$next_app"
}
trap cleanup_staging_app EXIT

export KVMFLOW_BUILD_ID="$build_id"
(
  cd apps/macos
  npm run tauri:macos:build -- --bundles app
)
scripts/sign-macos-app.sh "target/release/bundle/macos/kvmFlow.app"

mkdir -p "$package_root/data" "$package_root/logs" "$package_root/diagnostics"
if [[ ! -d "$package_root/kvmFlow.app" && -d "$previous_app" ]]; then
  mv "$previous_app" "$package_root/kvmFlow.app"
elif [[ -d "$previous_app" ]]; then
  rm -rf "$previous_app"
fi
cp -R "target/release/bundle/macos/kvmFlow.app" "$next_app"

had_previous_app=false
if [[ -d "$package_root/kvmFlow.app" ]]; then
  mv "$package_root/kvmFlow.app" "$previous_app"
  had_previous_app=true
fi

if ! mv "$next_app" "$package_root/kvmFlow.app"; then
  if [[ "$had_previous_app" == true ]]; then
    mv "$previous_app" "$package_root/kvmFlow.app"
  fi
  echo "failed to install the new macOS application at its stable path" >&2
  exit 1
fi

if ! node scripts/verify-package-layout.mjs "$package_root" macos; then
  rm -rf "$package_root/kvmFlow.app"
  if [[ "$had_previous_app" == true ]]; then
    mv "$previous_app" "$package_root/kvmFlow.app"
  fi
  exit 1
fi

rm -rf "$previous_app"
printf '%s\n' "$build_id" > "$package_root/build-id.txt"
printf '%s\n' "$package_root"
