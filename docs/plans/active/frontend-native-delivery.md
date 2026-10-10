# 前后端集成与 Windows 交付计划

> 执行方式：主集成工程师持续执行，独立代理并行只读核查与最终审查；遵循 verification-before-completion。

**目标**：保留前端共同窗口布局与实际 Previewer 后端，在已验证的 dev 提交生成可安装 Windows x64 测试包。
**架构**：保持 protocol/model.json 单一来源、src/ipc 唯一 UI 原生入口、Rust 会话/信任/构建/帧边界。原生能力缺失明确阻塞，正式程序没有测试替身。
**技术栈**：Tauri 2、Rust MSVC、React 18、TypeScript 6、Vite 6、Node >=24.18、pnpm 11.22.0。
**规格**：docs/product.md、docs/architecture.md；本次用户要求本地合并 dev、最终 dev Release 打包和 Windows 人工验收指南。

## 安全与审查重点

- 不修改两个来源 worktree，不丢弃原件、不推送、不删除 worktree。
- SDK 不重新分发；Windows x64、SDK 26.0.0.105 当前适配范围之外明确拒绝。
- 弱发现不能自动关联；执行信任独立核验；会话/构建代次不串线。
- 保留前端功能及回归；生成类型从语义合并后的协议重新生成。
- 验证安装包加载生产资源，无 Vite/worktree 运行依赖；缺 SDK 仍能启动。
- 构建失败旧画面禁用输入，停止/切换/重载释放自有资源。

## Task 1：可追溯的语义合并

文件：protocol/model.json、生成的两端 model、src-tauri/src/desktop.rs、src/App.tsx、src/Sheet.tsx、src/request-state.ts、src/styles.css、tests、tools、Harness 文档。

- [x] 检查所有本地分支、HEAD、共同祖先、worktree 与未提交修改。
- [x] 确认没有 dev/远程；从 main c090418 创建 dev 与 backup/dev-before-integration-20261010。
- [x] 建立 D:/Projects/Rust/arkdock-integration-20261010，integration/arkdock-frontend-native。
- [x] 正常合并前端历史；合并后端，重新生成两端协议解决摘要冲突。
- [x] 检查共享文件语义；后端合并提交 071d865，保留前端；修复工具链阻塞覆盖信任。
- [x] 冻结安装、完整 verify 退出 0，desktop lib 108 项退出 0。

## Task 2：真实 IPC、状态、窗口与 SDK 联调

文件：src/ipc/client.ts、src/frame-store.ts、src/PreviewStage.tsx、src/view-state.ts、tools/*smoke.mjs、tests/native。

- [x] 运行本次 dev 桌面和临时自有调试端口；来源 worktree 与用户其他应用保留。
- [ ] pnpm verify:ui、verify:joint、verify:windows，记录真实结果，不删断言。
- [ ] pnpm verify:native：真实样例构建、首帧、点击、.ets 更新、错误恢复与清理。
- [ ] 实际二进制 IPC 模式与兼容模式恢复检查；不把 CLI/fixture 当 UI 原生证明。
- [ ] 修复可复现集成错误，做最小回归；记录跨 DPI/未支持输入等限制。

## Task 3：文档与 dev 更新

文件：README.md、docs/status.md、docs/architecture.md、docs/native-preview/validation.md、docs/testing/windows-acceptance.md、docs/evidence/integration-delivery-2026-10-10.md。

- [x] 独立最终代码审查无新增缺陷；两源 HEAD 都为集成提交祖先，前端功能保留。
- [x] 文档记录基础 PASS、独立 SDK T1–T7 PASS 和 UI/窗口/native gate 的实际 FAIL。
- [x] 确认 dev 基线未变化，正常 fast-forward；只在自有隔离 worktree 检出 dev。

## Task 4：最终 dev 软件包与验收

文件：src-tauri/tauri.conf.json、tools/desktop.mjs（仅必要兼容修复）、artifacts/delivery-2026-10-10/。

- [x] 检出的 dev 完成 Release NSIS/MSI；文档最终提交后再次重建，manifest 锁定最终 SHA。
- [x] 核对 AMD64、0.1.0、生产前端/真实 IPC、NotSigned；最终 SHA-256/大小记录 manifest。
- [x] 实际 NSIS 安装/启动/关闭/无调试参数重启/SDK_MISSING/卸载 PASS，原生配置摘要不变。
- [x] 初版 0.1.0 NSIS 安装/启动/重启/SDK_MISSING/卸载 PASS，原生配置摘要不变；Apache许可版已构建及核对安装资源。
- [x] Apache-2.0 版 NSIS/MSI 构建 PASS；许可证元数据、NOTICE 与 LICENSE 安装资源已检查。
- [ ] Apache-2.0 安装版 WebView2 启动复测：当前有另一工作区同产品实例，HRESULT 0x8007139F / 101；该实例未触碰。

## Task 5：GitHub 发布

- [x] Apache-2.0 LICENSE、Cargo/npm SPDX 元数据和安装资源已提交到 dev。
- [ ] 目标仓库缺失：GitHub 账户 Wuysau 下 Wuysau/ArkDock 返回 404；本地未配置 remote。
- [ ] GitHub CLI 授权无效。用户需在 GitHub 创建空的公开仓库并于本机重新认证后再推送。
- [ ] 现有 merge history 作者使用非-noreply 邮箱；发布前确认是否保留完整历史，不自动改写提交。
- [ ] 记录产物关联 commit；交付指南不要求用户 Node/Rust/Vite。

## 执行记录

2026-10-10：主工作树 4ebb828，后端 7cf47d0，main c090418，共同祖先 aab0f5e。两个来源 worktree 保留。
主工作树未跟踪项为 AGENTS/CLAUDE/README 和现有 docs；核对后仅复制到集成工作树，原件不变。
后端 docs/status.md 与主前端状态将整合，历史仍由来源提交/证据保留。
Ruling：dev 原本不存在，使用已提交的 main c090418 作为新 dev 基线；恢复引用固定同一提交。
Ruling：集成允许正常 merge 保留历史；共享生成类型不手动拼接，从统一协议重新生成。

2026-10-10 更新：实际 UI 七组通过，指针边缘缩放失败；joint/native 在真实绑定前置失败。用户选择暂不提供鼠标验收桌面，优先安装包。保持未签名测试构建，不声明完整发布门禁通过。
独立真实 SDK CLI T1–T7 PASS（artifacts/native-preview/integration-20261010/report.json），源文件已恢复。
固定官方 NSIS/WiX 归档摘要核验，第三方许可资源和五个 MPL 源码归档已准备；vendor Rust 版权 HTML 保持原字节、以 .html.txt 作为资源。

最终实施：新增 native-tools 只隔离开发 CLI 安装目标，正式原生库未禁用；全套工具回归仍运行。独立审查确认 Tauri 不会重新发现被 required-features 排除的 src/bin。
实际包内红绿回归：旧包含 native-preview.exe 时退出 1，重建后同一安装断言退出 0。MSI 文件表仅主 exe 和许可资源，ALLUSERS=1，系统安装 SKIPPED。
剩余任务是空闲可见桌面的鼠标/React 原生/跨 DPI 验收；按用户要求先交付已验证基本安装/启动的测试包。
