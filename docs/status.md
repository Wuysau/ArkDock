# 状态索引

2026-10-10：前端 feat/joint-dock-layout@4ebb828 与后端 feat/native-preview-backend@7cf47d0
已正常合并至 integration/arkdock-frontend-native，来源 worktree 和本地未跟踪原件保持不变。
dev 原先不存在，基线 main@c090418；backup/dev-before-integration-20261010 保留恢复点。
dev 已正常 fast-forward 更新，在隔离工作树检出并生成 0.1.0 未签名 Windows x64 NSIS/MSI 测试包。
最终 dev/构建 SHA、实际大小与 SHA-256 以 artifacts/delivery-2026-10-10/manifest.json 为准。

- [当前集成交付计划](plans/active/frontend-native-delivery.md)
- [本次命令、退出码和证据](evidence/integration-delivery-2026-10-10.md)
- [Windows 安装版验收指南](testing/windows-acceptance.md)
- [上一轮共同布局](evidence/joint-layout-2026-10-10.md)
- [原生兼容与历史验证](native-preview/validation.md)

基础 verify PASS（37 Node、102 Rust 核心、8 原生 CLI、2 登记 CLI、1 watcher、6 持久化）。
desktop lib 108 项 PASS。已修复 SDK 阻塞覆盖有效工程信任的集成问题。
最终 desktop,native-tools --all-targets 共 125 项 PASS，两个 CLI 的全部测试保留。
本次独立后端真实 SDK T1–T7 PASS（首帧、点击、源码更新、失败恢复、停止）。
实际 WebView2 UI 七组 PASS，边缘鼠标缩放 FAIL；共同窗口绑定与 React 原生门禁在拖动前置 FAIL。
测试时有遮挡或焦点竞争；用户选择先生成安装包，暂不继续鼠标测试。
这些失败保留，不能宣布完整桌面/React 原生/发布门禁通过。双屏跨 DPI 仍 BLOCKED。

NSIS 专用目录安装、真实生产资源/Rust IPC、独立窗口、正常退出、无调试参数重启、SDK_MISSING 与卸载 PASS；
用户原有配置摘要不变，安装包仅主应用和许可资源，不包含开发 CLI/SDK。MSI 构建/内容核对 PASS，系统级安装 SKIPPED。
下一步：用户有空闲可见桌面时按指南复核边缘缩放、贴边、React 首帧和点击/刷新。
