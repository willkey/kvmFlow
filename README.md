# kvmFlow

kvmFlow 是一款局域网软件 KVM：使用 Windows 的键盘和鼠标控制一台 Mac。它只传输输入事件，不传输屏幕、音频或文件，也不是远程桌面软件。

> 当前 `0.1.0` 是首个开源预览版本，目标是验证 Windows 控制端到 macOS 被控端的基础连接、配对、跨屏和键鼠控制链路。请仅在可信局域网中使用，不要把监听端口暴露到互联网。

## 已实现

- Windows 手动输入 Mac 的 IPv4 地址和端口，Mac 监听端口可在 `1024-65535` 范围内修改。
- TLS 1.3 加密传输、双端六位码核对、长期设备身份和一对一可信设备持久化。
- 一秒心跳、固定五秒自动重连、断线后立即恢复 Windows 本机输入。
- Windows 原生键鼠捕获与屏蔽，macOS CoreGraphics 输入注入和辅助功能权限检查。
- Windows 目标显示器与进入边缘配置、自动跨屏、从 Mac 相反边缘返回。
- 键盘、鼠标移动、左右键、中键、侧键、滚轮和常用修饰键转换。
- `Ctrl+Alt+Shift+F12` 紧急返回 Windows。
- Windows 托盘、macOS 菜单栏、窗口关闭后后台运行。
- 结构化日志、诊断报告和持续资源监控；日志不记录实际按键内容、鼠标轨迹、完整配对码或设备私钥。

## 暂未实现

- 剪贴板同步、文件传输和跨设备拖放。
- 自动发现、主机名连接和 IPv6。
- 自定义键盘映射、按应用区分的键位方案和完整多语言界面。
- 多台 Mac、并发会话、自动更新和正式安装器更新链路。

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

本地开发签名不等同于 Apple Developer ID 签名或公证，不应作为面向其他用户的正式 macOS 发布签名。Windows 未签名构建也可能触发 SmartScreen 提示。

## 首次连接

1. 启动 Mac 应用，在“系统设置 -> 隐私与安全性 -> 辅助功能”中允许当前 `kvmFlow.app`，然后在应用内刷新权限。
2. 在 Mac 界面确认实际 IPv4 地址和监听端口；默认端口为 `45888`，发生冲突时应用会明确报错，不会静默换端口。
3. 启动 Windows 应用，输入 Mac 的 IPv4 地址和端口并连接。
4. 核对两端持续显示的六位配对码，分别确认后建立一对一信任关系。
5. 在 Windows 选择目标显示器和 Mac 所在边缘，然后通过该边缘进入 Mac；从 Mac 相反边缘返回 Windows。

## 本地数据与发布包

运行数据保存在应用同级目录，而不是写入可执行文件或 macOS `.app` 内部：

```text
kvmFlow/
  kvmFlow.exe 或 kvmFlow.app
  data/
  logs/
  diagnostics/
```

`data/device-identity.json` 包含未加密的设备私钥。macOS 会把文件权限限制为仅当前用户可读写，Windows 使用所在目录继承的访问控制。复制、共享或备份整个运行目录前，请先确认其中不包含身份、配对、日志或诊断数据。

运行过的目录不能直接作为发布包。请把干净版本导出到一个不存在的新目录：

```sh
node scripts/export-clean-package.mjs <运行目录> <新发布目录> <windows|macos>
```

导出器只复制应用和构建 ID，并创建空的 `data`、`logs`、`diagnostics` 目录。

## 安全与贡献

安全边界、漏洞报告方式和本地敏感文件说明见 [SECURITY.md](SECURITY.md)。参与开发前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)，版本变化见 [CHANGELOG.md](CHANGELOG.md)。

## 许可证

kvmFlow 以 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 双许可证发布，你可以任选其一使用。
