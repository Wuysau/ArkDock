# 可执行质量门禁

| 命令 | 内容 | 必要环境 |
| --- | --- | --- |
| pnpm test:portable | IPC、版本/帧显示、候选兜底、诊断与门禁分类回归 | Node 24，无 npm 依赖 |
| pnpm verify | 仓库格式、契约、Harness/能力结构、便携测试、Oxlint、TS、Vite、Rust fmt/clippy/test/build | npm 依赖、Rust，无鸿蒙 SDK |
| pnpm verify:native | 本次实际 SDK 构建、真实帧/点击/.ets 刷新、失败恢复及自有 Job 清理 | Windows 11/桌面/匹配 SDK/运行中的开发 ArkDock |
| pnpm verify:release | 基础 + 原生；都 PASS 才 Windows 打包 | 完整原生环境 |
| pnpm verify:windows | 实际鼠标、目标生命周期、轮廓、布局及 UIA 候选 | 正在运行的开发 ArkDock/WebView2 调试端口 |
| pnpm verify:ui | 真实 WebView2 主题持久化、系统媒体变化、自适应和导航 | 同上；不提供任何模拟原生画面 |
| pnpm run doctor | 显式目录/环境/工具版本诊断，无工程执行 | Node 24 |

基础聚合运行全部可用检查，并把结果写入 `artifacts/verify.json`。
pnpm 11 自身的 doctor 为内置命令，本项目诊断必须显式 `pnpm run doctor`。
普通编译工具的非零退出码一律 FAIL；只有 ArkDock 原生 gate 的 2 是 BLOCKED。
缺失可执行程序/权限为 BLOCKED。CI 不用 continue-on-error、删断言或禁用 lint
绕过失败；原生 gate 没有 mock、SKIPPED 成功或手填证据转绿路径。
原生预检使用 protocol/native-profile.json 的实测版本/文件摘要；随后必须执行
tools/native-smoke.mjs、退出 0，且收到本次时间范围内完整 D1/D2/C 验收记录。
native-support.json 的历史布尔字段不参与门禁。测试会修改/恢复自有真实 .ets，
暂时信任并最终撤销原先未信任的验收工程；不执行用户未授权的其他工程。

仓库 format 检查明确覆盖 LF、末尾换行、行末空白与 JSON 语法，
Rust 风格另由 `cargo fmt --check` 执行；不声称文本检查等同完整语言格式化。
Rust check/clippy/test/build 使用 --locked，版本下限检查不会把 Node 24.13 当成满足 >=24.18。
Oxlint 检查 TS/TSX 正确性，TypeScript 检查类型，Vite 构建前端。
结构检查限定 UI 原生入口/能力，不能替代运行期会话和参数校验。
IPC 生成比较两端文件及 source SHA256；Rust serde 拒绝未知字段。

Linux CI 只验证可移植 Rust 核心和前端。Windows CI 额外编译 desktop features，
仍不证明拖动、DPI 或 ArkUI。真正窗口/预览必须按 [runbook](../runbooks/windows-native-preview.md)
手测或完成真实驱动。原生 CI 需已配置的 Windows 自托管 runner，默认手动触发。

测试替身仅用于 cfg(test) 或 tests；正式后端只有 native-previewer。
不要将测试中的受控伪 PNG 帧计为产品原生画面。
