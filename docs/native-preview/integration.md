# 并行开发集成说明

工作树：`D:/Projects/Rust/arkdock-native-preview`。分支：
`feat/native-preview-backend`。共同基线：
`aab0f5e2c579e8759bb3497d642d8836f3094af1`（main 的已提交 checkpoint）。
原工作树 `D:/Projects/Rust/ArkDock` 的前端和文档未提交修改未复制、覆盖或提交。

## 本分支范围

- `src-tauri/src/preview/build.rs`：预览任务参数和实际构建图完成回执。
- `src-tauri/src/preview/environment.rs`：只读版本、摘要、架构、DLL/资源诊断。
- `src-tauri/src/preview/frames.rs`：JPEG40 解析、二进制帧、背压、计量与兼容转换。
- `src-tauri/src/preview.rs`、`preview/windows.rs`：现有真实适配器接入以上模块。
- `src-tauri/src/scheduler.rs`：二进制帧的成功构建/代次/顺序检查。
- `src-tauri/src/bin/arkdock-native-preview.rs`：限制于自有 ArkTS 示例的独立验收。
- `docs/native-preview/`、`docs/status.md`：研究、契约、环境、证据和本任务状态。

潜在共享文件的最小修改均在本分支：`src-tauri/src/desktop.rs` 修复配置指纹
失效后的取消，并注册二进制读取、选择/解除模式和 WebView 重载清理；
`protocol/model.json`、`protocol/native-profile.json` 更新契约/依据；
`tools/generate-ipc.mjs` 生成命令返回类型，`src-tauri/src/ipc/model.rs` 和
`src/ipc/model.ts` 是生成结果；`src/ipc/client.ts` 仅新增类型化二进制入口，
保留 UI 已依赖的快照调用签名；`tests/portable/contracts.test.mjs` 核验返回契约；
`.gitignore` 忽略真实 module 预览构建目录。未修改 React 组件、动画、视觉样式、
根 package scripts、Cargo workspace、`src-tauri/src/lib.rs`、AGENTS 或 CLAUDE。

## 安全集成顺序

1. 前端会话先独立保存/提交自己的修改；比较双方相对共同基线的实际 diff。
2. 在集成分支合并此后端分支，逐项审查上述共享文件；本会话不自动合并或推送。
3. 保持 `protocol/model.json` 为唯一字段来源，运行 IPC 生成与契约检查。
4. 默认 `arkdock:frame` 兼容事件可继续使用。前端接入原始 JPEG 时，遵循
   [frontend-contract.md](frontend-contract.md)，使用唯一 IPC helper、有界轮询、
   版本校验和 Canvas/Blob 资源释放；不要把帧加入 React 全局状态。
5. 合并后的桌面应用重新运行实际 native gate、前端交互和二进制 ArrayBuffer
   联调；确认消费者卸载、页面重载、停止/重新信任/换工程恢复兼容入口。

## 仍须集成后验证

独立 Rust 驱动证明后端调用 SDK 的真实链路，不能代替 Tauri/WebView2 到实际
前端 Canvas 的二进制传输、DPI/letterbox 点击和 UI 旧画面展示。当前源码更新
使用增量 `PreviewBuild` 后启动新 Previewer；组件状态重置。未证明状态保留
热重载、键盘/IME、滑动、旋转、其他 SDK、WSL 或远程预览。

共同基线没有提交原工作树中的 Harness 入口和大部分 docs，基础聚合门禁
会报告缺失；基线 `tools/ui-smoke.mjs` 还有 8 个 lint 错误。不得通过复制覆盖
另一会话文档或删断言把本分支门禁变绿。具体命令与状态见
[validation.md](validation.md)。保留本工作树、分支和本地 artifacts 供审查与复现。
