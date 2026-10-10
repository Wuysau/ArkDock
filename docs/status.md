# 状态索引

2026-10-10：前端 feat/joint-dock-layout@4ebb828 与后端 feat/native-preview-backend@7cf47d0
已正常合并至 integration/arkdock-frontend-native，来源 worktree 和本地未跟踪原件保持不变。
dev 原先不存在，基线 main@c090418；backup/dev-before-integration-20261010 保留恢复点。

- [当前集成交付计划](plans/active/frontend-native-delivery.md)
- [本次命令、退出码和证据](evidence/integration-delivery-2026-10-10.md)
- [Windows 安装版验收指南](testing/windows-acceptance.md)
- [上一轮共同布局](evidence/joint-layout-2026-10-10.md)
- [原生兼容与历史验证](native-preview/validation.md)

基础 verify PASS（37 Node、102 Rust 核心、8 原生 CLI、2 登记 CLI、1 watcher、6 持久化）。
desktop lib 108 项 PASS。已修复 SDK 阻塞覆盖有效工程信任的集成问题。
本次独立后端真实 SDK T1–T7 PASS（首帧、点击、源码更新、失败恢复、停止）。
实际 WebView2 UI 七组 PASS，边缘鼠标缩放 FAIL；共同窗口绑定与 React 原生门禁在拖动前置 FAIL。
测试时有遮挡或焦点竞争；用户选择先生成安装包，暂不继续鼠标测试。
这些失败保留，不能宣布完整桌面/React 原生/发布门禁通过。双屏跨 DPI 仍 BLOCKED。

下一步：完成最终 dev 未签名 NSIS/MSI 构建与启动核验，更新产物清单；
用户有空闲可见桌面时按指南复核边缘缩放、贴边、React 首帧和点击/刷新。
