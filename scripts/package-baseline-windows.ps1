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
    $packageRoot = Join-Path $repoRoot "dist/windows/current"
    $executablePath = Join-Path $packageRoot "kvmFlow.exe"
    $nextExecutable = Join-Path $packageRoot ".kvmFlow.exe.next-$buildId"
    $previousExecutable = Join-Path $packageRoot ".kvmFlow.exe.previous"

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

    New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
    foreach ($directory in @("data", "logs", "diagnostics")) {
        New-Item -ItemType Directory -Path (Join-Path $packageRoot $directory) -Force | Out-Null
    }
    if (!(Test-Path -LiteralPath $executablePath) -and (Test-Path -LiteralPath $previousExecutable)) {
        Move-Item -LiteralPath $previousExecutable -Destination $executablePath
    }
    if (Test-Path -LiteralPath $previousExecutable) {
        Remove-Item -LiteralPath $previousExecutable -Force
    }
    Copy-Item (Join-Path $buildRoot "target/release/kvmFlow.exe") $nextExecutable

    $hadPreviousExecutable = Test-Path -LiteralPath $executablePath
    try {
        if ($hadPreviousExecutable) {
            Move-Item -LiteralPath $executablePath -Destination $previousExecutable
        }
        Move-Item -LiteralPath $nextExecutable -Destination $executablePath

        node (Join-Path $buildRoot "scripts/verify-package-layout.mjs") $packageRoot windows
        if ($LASTEXITCODE -ne 0) {
            throw "Windows package layout verification failed"
        }
    }
    catch {
        if (Test-Path -LiteralPath $nextExecutable) {
            Remove-Item -LiteralPath $nextExecutable -Force -ErrorAction SilentlyContinue
        }
        if (Test-Path -LiteralPath $executablePath) {
            Remove-Item -LiteralPath $executablePath -Force -ErrorAction SilentlyContinue
        }
        if ($hadPreviousExecutable -and (Test-Path -LiteralPath $previousExecutable)) {
            Move-Item -LiteralPath $previousExecutable -Destination $executablePath
        }
        throw "failed to install the new Windows executable; close kvmFlow and retry: $($_.Exception.Message)"
    }

    if (Test-Path -LiteralPath $previousExecutable) {
        Remove-Item -LiteralPath $previousExecutable -Force
    }
    Set-Content -LiteralPath (Join-Path $packageRoot "build-id.txt") -Value $buildId
    Write-Output $packageRoot
}
finally {
    Pop-Location
    if ($stagingRoot -and (Test-Path -LiteralPath $stagingRoot)) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
