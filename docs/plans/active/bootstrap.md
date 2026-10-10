# ArkDock 当前实现计划

目标：接续已有实现，在当前 Windows 11 原生环境完成可验证的独立桌面、发现、
信任与调度；合法 SDK 的真实闭环仍是产品完成条件。
规格：[product](../../product.md)；架构：[architecture](../../architecture.md)。
主流程在当前目录实施，保留已有文件。不初始化缺失的 Git 元数据，不推送或发布。

## 当前检查点（2026-10-09 Windows 接续）

- [x] 重新核验原生 Windows、工具链、权限与用户 SDK 安装状态。
- [x] 冻结前端安装并通过 pnpm 供应链校验，生成并交付 Cargo.lock。
- [x] 修复真实 lint/typecheck/Rust desktop 编译错误与缺失打包图标。
- [x] 项目内锁定官方 Tauri CLI；修复 dev 监听 Rust 锁文件导致的 EBUSY。
- [x] 真正运行基础 verify：前端构建、Rust fmt/clippy/test/build、Windows desktop check/clippy。
- [x] 启动实际 Tauri/WebView2；实际鼠标拖动绑定、跟随、焦点、最小化/恢复、关闭。
- [x] 四边无激活穿透轮廓；完整 WINDOWPLACEMENT 正常/最大化条件恢复。
- [x] 受限 monorepo 多应用发现，工作区/模块/环境身份保持独立。
- [x] 只读 UIA 当前选中标签元数据；不能证明活动工程范围时仍需候选选择。
- [x] 完成独立审查提出的异步解绑竞态和配置分配预算修复及回归。
- [ ] 最终 doctor/base/Windows/native/release 命令及逐 ID 文档一致性检查。
- [ ] A2 权限差异、快速 HWND 复用压力；A3 不同 DPI 双屏及负坐标真实验收。
- [ ] B2 认证登记、进程参数可信范围与宿主活动工程证明；当前无默认自动关联来源。
- [x] D1/D2 合法 SDK、真实工程、无 GUI 构建、Previewer 帧/输入/源码刷新与实际原生驱动。
- [x] 自有 Windows Job、实际失败旧画面/输入拒绝/源码修复恢复。
- [ ] 审查后撤销信任/重新授权实际回归、设置残片重试与最终全量核验。
- [ ] 原生 PASS 后才执行 E2 正式 Windows 打包与发布门禁。

当前基础基线：15 Node、60 Rust 核心、2 helper、6 持久化与 1 OS watcher 回归通过；新增回归后
以 [本次验证](../../evidence/verification-2026-10-09.md) 最终统计为准。
真实桌面驱动是 A/B 验收工具，不充当 D 原生预览驱动或画面替身。

## 环境与可逆选择

Windows 11 家庭中文版 10.0.26300 x64，PowerShell 7.6.6；Rust/Cargo 1.94.1
MSVC target，MSVC 14.44.35207，Windows SDK 10.0.26100.0，WebView2 154.0.4258.62。
单屏物理 2880×1800、192 DPI。双屏原生案例 BLOCKED；未执行的压力/权限案例 SKIPPED。

已有 Node 24.13.0 低于 package.json 的 >=24.18.0 下限。
官方 portable Node 24.21.0 经官方 SHA256 核验，置于 D:\CodingTools；
pnpm 11.22.0 与项目 packageManager 一致，官方包放在 D:\CodingTools\ArkDock-pnpm。
仅本次进程调整 PATH，不修改全局配置。Tauri CLI @tauri-apps/cli 2.12.1 固定项目依赖。
Windows 绑定 windows-sys 0.59.0 与 UIA windows 0.62.2 固定版本并已编译。

当前没有有效 Git 元数据，git status 退出 128；不能提供 Git diff/提交证据。
用户已自行安装 D:\CodingTools\HarmonyOS\command-line-tools：Command Line Tools
26.0.0.851、HarmonyOS 26.0.0.105 / API 26、Hvigor 6.26.8、ohpm 26.0.0.630。
未代登录/接受许可、不重新分发 SDK。已核验 Previewer 签名/摘要与当前具体协议，
真实 native-counter 未签名 HAP、原生帧/点击/源码刷新见 [SDK 证据](../../evidence/sdk-26-preview.md)。
目前没有安装包；其他商业版本不能据此启用。

## 下一条可执行步骤

完成当前审查修复与最终门禁；匹配 SDK 后每次 native gate 实际运行 D1/D2 断言，
全部前置通过再打包，并记录真实路径、SHA256 与逐 ID 未测范围。
不猜商业 SDK 与 OpenHarmony 兼容，不编辑 native-support.json 使门禁变绿。
未信任工程始终不执行；进程树清理已接入仅覆盖自有子进程的 Windows Job。

上次受限 WSL 的缺 Cargo/网络/互操作记录仅为历史，不能作为本次阻塞理由。
本计划保持 active，A3/B2 等剩余范围仍不能归档为全部产品验收。
