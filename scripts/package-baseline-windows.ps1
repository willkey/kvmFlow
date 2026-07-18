$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$stagingRoot = $null
Push-Location $repoRoot

try {
    # The archive makes untracked transfer files irrelevant; filemode avoids false changes on SMB.
    $dirtyPaths = @(git -c "safe.directory=*" -c "core.filemode=false" status --porcelain --untracked-files=no)
    if ($LASTEXITCODE -ne 0) {
        throw "failed to inspect the Git worktree"
    }
    if ($dirtyPaths.Count -ne 0) {
        $dirtyDetails = $dirtyPaths -join [Environment]::NewLine
        throw "refusing to package from a dirty Git worktree:$([Environment]::NewLine)$dirtyDetails"
    }

    $timestamp = (Get-Date).ToUniversalTime().ToString("yyyyMMdd-HHmmss")
    $shortShaOutput = git -c "safe.directory=*" -c "core.filemode=false" rev-parse --short=12 HEAD
    if ($LASTEXITCODE -ne 0) {
        throw "failed to resolve the Git commit"
    }
    $shortSha = ($shortShaOutput | Out-String).Trim()
    $buildId = "$timestamp-$shortSha"
    $packageRoot = Join-Path $repoRoot (Join-Path "dist/windows" $buildId)

    if (Test-Path -LiteralPath $packageRoot) {
        throw "package destination already exists: $packageRoot"
    }

    $stagingRoot = Join-Path $env:TEMP "kvmflow-build-$buildId-$PID"
    $archivePath = Join-Path $stagingRoot "source.zip"
    $buildRoot = Join-Path $stagingRoot "source"
    New-Item -ItemType Directory -Path $stagingRoot | Out-Null

    git -c "safe.directory=*" -c "core.filemode=false" archive --format=zip "--output=$archivePath" HEAD
    if ($LASTEXITCODE -ne 0) {
        throw "failed to create a clean source archive"
    }
    New-Item -ItemType Directory -Path $buildRoot | Out-Null
    Expand-Archive -LiteralPath $archivePath -DestinationPath $buildRoot

    Push-Location $buildRoot
    try {
        npm ci
        if ($LASTEXITCODE -ne 0) {
            throw "frontend dependency installation failed"
        }

        $env:KVMFLOW_BUILD_ID = $buildId
        npm run tauri:windows:build
        if ($LASTEXITCODE -ne 0) {
            throw "Windows Tauri build failed"
        }
    }
    finally {
        Pop-Location
    }

    New-Item -ItemType Directory -Path (Split-Path -Parent $packageRoot) -Force | Out-Null
    New-Item -ItemType Directory -Path $packageRoot | Out-Null
    Copy-Item (Join-Path $buildRoot "target/release/kvmFlow.exe") (Join-Path $packageRoot "kvmFlow.exe")
    foreach ($directory in @("data", "logs", "diagnostics")) {
        New-Item -ItemType Directory -Path (Join-Path $packageRoot $directory) | Out-Null
    }

    node (Join-Path $buildRoot "scripts/verify-package-layout.mjs") $packageRoot windows
    if ($LASTEXITCODE -ne 0) {
        throw "Windows package layout verification failed"
    }
    Write-Output $packageRoot
}
finally {
    Pop-Location
    if ($stagingRoot -and (Test-Path -LiteralPath $stagingRoot)) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
