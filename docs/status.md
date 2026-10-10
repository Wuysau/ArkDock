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

初版 NSIS 测试包的私有目录安装、真实生产资源/Rust IPC、独立窗口、退出/重启、SDK_MISSING 与卸载 PASS；
用户原有配置摘要不变。Apache-2.0 版安装包包含 LICENSE，安装资源摘要核验 PASS；
许可证版 WebView2 启动复测 BLOCKED：另一同产品实例正运行，创建 WebView2 返回 0x8007139F，退出 101；未触碰该实例。
MSI 构建 PASS，系统级安装 SKIPPED。最新 x64 安装包/哈希和授权发布阻塞见交付清单。
下一步：用户有空闲可见桌面时按指南复核边缘缩放、贴边、React 首帧和点击/刷新。

## GitHub 授权发布

本地 \`dev\` 已提交 Apache-2.0 LICENSE，并同步 README、Cargo/npm 元数据和 Tauri 安装资源。Apache-2.0 允许符合条款的商业使用，不会阻止非法使用者。

推送尚未发生：本地没有远程；账户 \`Wuysau\` 搜索不到 \`Wuysau/ArkDock\`，仓库 API 返回 404；本机 \`gh\` 登录令牌无效。当前集成浏览器也停在 GitHub 登录页。准备目标为新的公开 \`Wuysau/ArkDock\` 仓库。

推送之前还需 GitHub 登录和空仓库。现有提交使用非 GitHub noreply 地址；保留完整合并历史推送会公开该提交作者地址，尚未重写历史。完成后不要通过聊天发送凭据。

Apache-2.0 版 Release Build 已生成，安装器包含 LICENSE 且安装资源哈希匹配；安装 smoke 遇到另一个工作区正在运行的 ArkDock，WebView2 创建窗口返回 HRESULT 0x8007139F / exit 101。未关闭该进程。最新许可证版本的窗口启动状态为 BLOCKED，旧版本安装启动 PASS 不覆盖此结果。
