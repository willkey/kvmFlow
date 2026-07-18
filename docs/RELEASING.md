# 发布 kvmFlow

本文面向维护者。普通用户请从 GitHub Releases 下载正式安装包，不要使用 Actions 中标有 `UNSIGNED` 或 `UNNOTARIZED` 的候选包。

## 发布门禁

正式 Release 必须同时满足：

1. 版本号与 `CHANGELOG.md` 一致，公开目录差异已完整审查。
2. `main` 的 Windows 与 macOS CI 均通过。
3. Windows NSIS 安装包已使用代码签名证书签名，签名状态为 `Valid`。
4. macOS Universal DMG 已使用 Developer ID Application 签名、完成 Apple 公证并附加公证票据。
5. 安装包的 SHA-256 校验文件正确。
6. 新标签上的 Windows 与 macOS CI 均通过，然后才创建 GitHub Release。

Release 与标签视为不可变。发现问题时修复并增加补丁版本，不覆盖旧版本。

## GitHub Secrets

在公开仓库的 `Settings -> Secrets and variables -> Actions` 中配置：

| Secret | 用途 |
| --- | --- |
| `WINDOWS_CERTIFICATE` | Base64 编码的 Windows PFX 代码签名证书 |
| `WINDOWS_CERTIFICATE_PASSWORD` | PFX 密码 |
| `APPLE_CERTIFICATE` | Base64 编码的 Developer ID Application P12 证书 |
| `APPLE_CERTIFICATE_PASSWORD` | P12 密码 |
| `APPLE_ID` | Apple 开发者账号 |
| `APPLE_PASSWORD` | Apple 专用密码 |
| `APPLE_TEAM_ID` | Apple Team ID |
| `KEYCHAIN_PASSWORD` | CI 临时钥匙串密码，使用随机强密码 |

证书与密码不得写入仓库、Issue、日志或 Release。Tauri 的官方说明见 [macOS 签名](https://v2.tauri.app/distribute/sign/macos/) 和 [Windows 签名](https://v2.tauri.app/distribute/sign/windows/)。

## 发布步骤

1. 在开发仓库更新代码、版本号和 `CHANGELOG.md`，运行：

   ```sh
   npm ci
   npm run release:check -- X.Y.Z
   npm run lint
   npm run typecheck
   npm test
   npm run build
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

2. 按精确公开文件清单同步，检查公开仓库完整差异，提交并推送 `main`。
3. 等待 `main` CI 全绿。
4. 在 GitHub 的 `Actions -> Release -> Run workflow` 中选择 `main`，输入版本号，先保持 `publish=false`。
5. 下载 Actions 产出的 Windows 和 macOS 候选包，在真实设备上完成启动、安装、权限、配对、跨屏、紧急返回和卸载验收。候选包不进入公开 Release。
6. 签名 Secrets 已配置且候选验收通过后，再运行同一工作流，设置 `publish=true` 并填写公开发布说明。
7. 工作流会签名、公证、校验安装包，创建标签，等待标签 CI，通过后才创建包含安装包和 SHA-256 文件的正式 Release。

## 正式 Release 资产

- `kvmFlow_<version>_windows_x64-setup.exe`
- `kvmFlow_<version>_windows_x64-setup.exe.sha256`
- `kvmFlow_<version>_macos_universal.dmg`
- `kvmFlow_<version>_macos_universal.dmg.sha256`

Windows 安装器由 NSIS 生成；macOS DMG 同时支持 Apple Silicon 和 Intel。GitHub Actions 候选资产只用于发布前测试，不代表正式分发版本。
