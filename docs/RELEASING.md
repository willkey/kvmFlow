# 发布 kvmFlow

本文面向维护者。普通用户应从 GitHub Releases 下载，不要使用 Actions 中的短期候选包。

发布分为两种模式：

- 未签名预览版：允许公开发布，但资产名必须保留 `UNSIGNED` / `UNNOTARIZED`，README 与 Release 说明必须醒目标注系统拦截和安全风险。
- 签名版：Windows 使用代码签名；macOS 使用 Developer ID Application 签名、Apple 公证并附加票据，是推荐的公开发布方式。

## 发布门禁

所有 Release 必须同时满足：

1. 版本号与 `CHANGELOG.md` 一致，公开目录差异已完整审查。
2. `main` 的 Windows 与 macOS CI 均通过。
3. 安装包的 SHA-256 校验文件正确。
4. 新标签上的 Windows 与 macOS CI 均通过，然后才创建 GitHub Release。
5. 未签名预览版的文件名、README 和 Release 说明均包含明确警告。

当 `sign=true` 时还必须验证 Windows 签名状态为 `Valid`，并验证 macOS Developer ID 签名、公证和票据。

Release 与标签视为不可变。发现问题时修复并增加补丁版本，不覆盖旧版本。

## GitHub Secrets

签名版需要在公开仓库的 `Settings -> Secrets and variables -> Actions` 中配置；未签名预览版不需要这些 Secrets：

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
4. 在 GitHub 的 `Actions -> Release -> Run workflow` 中选择 `main`，输入版本号，先保持 `publish=false` 和 `sign=false`。
5. 下载 Actions 产出的 Windows 和 macOS 候选包，在真实设备上完成启动、安装、权限、配对、跨屏、紧急返回和卸载验收。候选包不进入公开 Release。
6. 发布未签名预览版时，设置 `publish=true`、`sign=false` 并填写公开发布说明；必须提前获得项目负责人的明确许可。
7. 发布签名版时，确认 Secrets 已配置，设置 `publish=true`、`sign=true` 并填写公开发布说明。
8. 工作流会校验安装包，创建标签，等待标签 CI；通过后才创建包含安装包和 SHA-256 文件的 Release。未签名模式会自动在标题和说明中添加警告。

## Release 资产

签名版：

- `kvmFlow_<version>_windows_x64-setup.exe`
- `kvmFlow_<version>_windows_x64-setup.exe.sha256`
- `kvmFlow_<version>_macos_universal.dmg`
- `kvmFlow_<version>_macos_universal.dmg.sha256`

Windows 安装器由 NSIS 生成；macOS DMG 同时支持 Apple Silicon 和 Intel。GitHub Actions 候选资产只用于发布前测试，不代表正式分发版本。

未签名预览版：

- `kvmFlow_<version>_windows_x64-setup-UNSIGNED.exe`
- `kvmFlow_<version>_windows_x64-setup-UNSIGNED.exe.sha256`
- `kvmFlow_<version>_macos_universal-UNNOTARIZED.dmg`
- `kvmFlow_<version>_macos_universal-UNNOTARIZED.dmg.sha256`
