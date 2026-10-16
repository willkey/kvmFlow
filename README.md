# kvmFlow

kvmFlow 是一款局域网软件 KVM：使用 Windows 的键盘和鼠标控制一台 Mac，并在两端同步剪贴板。它不传输屏幕或音频，也不是远程桌面软件。

> 当前源码候选版本为 `0.2.2`，最新公开预览版仍为 `0.2.1`。公开安装包请从 [GitHub Releases](https://github.com/willkey/kvmFlow/releases) 下载。请仅在可信局域网中使用，不要把监听端口暴露到互联网。

## 已实现

- Windows 手动输入 Mac 的 IPv4 地址和端口，Mac 监听端口可在 `1024-65535` 范围内修改。
- 支持局域网自动发现：Mac 发布实际监听端口，Windows 展示可用设备；首次仍需双端核对配对码，自动发现模式会跟随已配对 Mac 的有效新地址。手动 IP 连接入口保留，手动模式不会被发现结果覆盖。
- TLS 1.3 加密传输、双端六位码核对、长期设备身份和一对一可信设备持久化。
- 一秒心跳、固定五秒自动重连、网络变化与睡眠唤醒后立即重试，断线后立即恢复 Windows 本机输入。
- Windows 原生键鼠捕获与屏蔽，macOS IOHID/CoreGraphics 输入注入和辅助功能权限检查。
- Windows 目标显示器与进入边缘配置、自动跨屏、从 Mac 相反边缘返回。
- 通用 HID 按键状态传输、Windows 到 macOS 修饰键策略、鼠标移动、左右键、中键、侧键和滚轮；组合键不依赖逐条快捷键规则。
- 默认双向同步文本、HTML、RTF、PNG 图片、文件和嵌套文件夹；剪贴板与控制使用相互独立、绑定同一可信会话的 TLS 通道。
- Windows 和 Mac 各自持久化最近 20 条剪贴板历史；点击 Windows 托盘或 Mac 菜单栏图标即可快速查看并重新复制，Windows 主界面另提供失效提示和清空。
- Windows 支持关闭跨屏控制、前台全屏保护和显示器布局冲突检测；剪贴板同步不依赖跨屏控制开关。
- Mac 可以紧急停止当前控制，Windows 会立即关闭边缘探测并恢复本机输入。
- 两端提供“重置按键状态”，释放远端按键、鼠标按钮并返回 Windows；Windows 同时重装输入钩子，随后可以重新跨屏。
- `Ctrl+Alt+Shift+F12` 紧急返回 Windows。
- Windows 托盘、macOS 菜单栏、窗口关闭后后台运行。
- Windows 与 macOS 可选择开机自动启动；登录后只显示托盘或菜单栏，不主动打开主窗口。
- 结构化日志、诊断报告、持续资源监控和 KVM 验收进度；日志不记录实际按键内容、鼠标轨迹、完整配对码或设备私钥。

## 暂未实现

- 跨设备直接拖放；当前文件同步通过系统剪贴板完成。
- 主机名直连和 IPv6。
- 自定义键盘映射、按应用区分的键位方案和完整多语言界面。
- 多台 Mac、并发会话、自动更新和正式安装器更新链路。

## 下载与安装

请从 [GitHub Releases](https://github.com/willkey/kvmFlow/releases) 下载最新公开版本。当前公开的 `v0.2.1` 是未签名预览版，会提供：

- Windows 10/11（x64）：`kvmFlow_0.2.1_windows_x64-setup-UNSIGNED.exe`。
- macOS 12 或更高版本（Apple Silicon 与 Intel）：`kvmFlow_0.2.1_macos_universal-UNNOTARIZED.dmg`。

> [!WARNING]
> `v0.2.1` 的 Windows 安装器没有代码签名；macOS DMG 仅使用 ad-hoc 签名，没有 Apple Developer ID 签名和公证。因此 Windows SmartScreen / Smart App Control 或 macOS 安全机制可能阻止安装或打开。请只从本项目 GitHub Releases 下载，并先核对 SHA-256；不能接受该风险时，请等待后续签名版本。

Windows 出现 Microsoft Defender SmartScreen 提示时，只有确认下载来源和 SHA-256 无误后才继续；若界面提供“更多信息 -> 仍要运行”，可自行选择。启用了 Smart App Control 的设备可能直接拦截未签名应用且不提供单个应用放行，此时建议等待签名版本，不要为了安装本软件关闭系统级保护。详见 [Microsoft Smart App Control 说明](https://support.microsoft.com/en-US/Windows/Security/Threat-Malware-Protection/smart-app-control-frequently-asked-questions)。

macOS 首次打开时若被阻止，请先尝试打开一次，再进入“系统设置 -> 隐私与安全性”，在“安全性”区域确认来源后选择“仍要打开”。绕过系统保护会增加风险，只应对从本仓库下载并完成校验的安装包操作。详见 [Apple 官方说明](https://support.apple.com/guide/mac-help/open-an-app-by-overriding-security-settings-mh40617/mac)。

下载后可使用同名 `.sha256` 文件核对完整性：

```powershell
certutil -hashfile .\kvmFlow_0.2.1_windows_x64-setup-UNSIGNED.exe SHA256
```

```sh
shasum -a 256 kvmFlow_0.2.1_macos_universal-UNNOTARIZED.dmg
```

## 项目结构

```text
apps/
  windows/             Windows Tauri 应用与 React 界面
  macos/               macOS Tauri 应用与 React 界面
crates/
  domain/              状态、事件和策略
  engine/              Actor 运行时与副作用编排
  protocol/            线路消息和帧协议
  transport/           TLS、配对、会话和配置存储
  diagnostics/         日志、资源采样和诊断报告
  platform-api/        平台抽象
  platform-windows/    Windows 原生输入实现
  platform-macos/      macOS 原生输入实现
packages/
  ui-contracts/        两端界面共享的类型契约
```

界面只渲染引擎快照并发送命令。Rust Engine Actor 是运行时状态的唯一所有者，窗口刷新或重建不会启动第二套网络或输入运行时。

## 开发环境

- Rust `1.96.0`，由 `rust-toolchain.toml` 固定。
- Node.js `22.14.x` 和 npm `10.9.2`。
- Windows 原生构建需要 Visual Studio 2022 C++ Build Tools 和 WebView2。
- macOS 原生构建需要 Xcode Command Line Tools。

安装依赖：

```sh
npm ci
```

运行完整质量检查：

```sh
npm run lint
npm run typecheck
npm test
npm run build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 本机运行与构建

```sh
npm run tauri:windows:dev
npm run tauri:windows:build
npm run tauri:macos:dev
npm run tauri:macos:build
```

Windows 发布包必须在 Windows 上运行：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package-baseline-windows.ps1
```

macOS 首次本地打包前，可以创建仅供本机开发使用的签名身份，然后生成固定路径的测试应用：

```sh
scripts/setup-local-macos-signing.sh
scripts/package-baseline-macos.sh
```

本地签名身份的私钥和证书保存在项目目录外的 `~/Library/Application Support/kvmFlow Development Signing/`。打包脚本使用固定版本的 `rcodesign` 直接读取 PEM 文件，不导入或信任任何钥匙串证书；连续构建复用同一私钥，不会因签名身份变化反复重置辅助功能授权。首次使用需安装 `cargo install apple-codesign --version 0.29.0 --locked`。

本地开发签名不等同于 Apple Developer ID 签名或公证，不应作为面向其他用户的正式 macOS 发布签名。Windows 未签名构建也可能触发 SmartScreen 提示。

维护者的候选包验收、签名配置和正式发布步骤见 [docs/RELEASING.md](docs/RELEASING.md)。

## 首次连接

1. 启动 Mac 应用，在“系统设置 -> 隐私与安全性 -> 辅助功能”中允许当前 `kvmFlow.app`，然后在应用内刷新权限。
2. 在 Mac 界面确认实际 IPv4 地址和监听端口；默认端口为 `45888`，发生冲突时应用会明确报错，不会静默换端口。
3. 启动 Windows 应用，可在“局域网设备”中选择 Mac；也可以输入 Mac 的 IPv4 地址和端口连接。自动发现需要同一局域网允许 mDNS（UDP 5353），并允许应用的系统局域网权限；网络隔离或多播被阻止时使用手动连接。
4. 核对两端持续显示的六位配对码，分别确认后建立一对一信任关系。
5. 在 Windows 选择目标显示器和 Mac 所在边缘，然后通过该边缘进入 Mac；从 Mac 相反边缘返回 Windows。

`0.2.2` 仍使用 V4 协议，兼容现有 `0.2.0` / `0.2.1` 配对与配置数据；建议两端同时更新以获得全部修复。配对成功后，两端系统剪贴板默认自动同步；复制文件或文件夹会先完整传输到远端缓存，再写入远端剪贴板。

## 本地数据与发布包

Windows 运行数据保存在 EXE 同级目录。macOS 安装目录只保留单个 `kvmFlow.app`，运行数据保存在当前用户的标准应用数据目录，不写入 `.app` 包：

```text
Windows/
  kvmFlow.exe
  data/
  logs/
  diagnostics/

macOS/
  /Applications/kvmFlow.app
  ~/Library/Application Support/com.kvmflow.endpoint/
    data/
    logs/
    diagnostics/
```

`data/device-identity.json` 包含未加密的设备私钥。两端的 `data/clipboard-history/` 会保存最近 20 条剪贴板内容，`data/clipboard-cache/` 可能保存已接收的图片、文件和文件夹。这些数据均未额外加密。macOS 会把身份文件权限限制为仅当前用户可读写，Windows 使用所在目录继承的访问控制。复制、共享或备份整个运行目录前，请先确认其中不包含身份、配对、剪贴板、日志或诊断数据。

文件和文件夹传输没有产品级容量上限。只与可信设备配对，并确保接收端有足够磁盘空间；大文件会一直占用缓存直到被历史和缓存清理策略淘汰。

运行过的目录不能直接作为发布包。请把干净版本导出到一个不存在的新目录：

```sh
node scripts/export-clean-package.mjs <运行目录> <新发布目录> <windows|macos>
```

导出器在 macOS 只复制 `kvmFlow.app`；Windows 会复制 EXE 和构建 ID，并创建空的 `data`、`logs`、`diagnostics` 目录。

## 安全与贡献

安全边界、漏洞报告方式和本地敏感文件说明见 [SECURITY.md](SECURITY.md)。参与开发前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)，版本变化见 [CHANGELOG.md](CHANGELOG.md)。

## 许可证

kvmFlow 以 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 双许可证发布，你可以任选其一使用。
