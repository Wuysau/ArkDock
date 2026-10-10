# 原生后端工作树状态

本工作树仅承载 Native Preview Backend 实施任务；共同基线为 `aab0f5e`。
当前计划：[native-preview/plan.md](native-preview/plan.md)。

SDK 26.0.0.105 / Windows x64 的独立真实后端 T1–T7 PASS：首帧、原生计数
点击、实际 .ets 监听更新、编译错误/恢复、停止和实测计量，见
[验证记录](native-preview/validation.md)。PreviewBuild 增量产物替代完整 HAP，
二进制 JPEG 契约和最小 IPC 接口已交付，刷新仍重启运行时并重置状态。

Rust 111 项、Node 31 项、fmt/clippy/TS/构建通过。完整 `pnpm verify` 仍 FAIL：
共同提交缺少原工作树未提交的 Harness 入口，且基线 UI smoke 有 8 项 lint 错误。
前端联调/发布为未执行，不能宣称完整 ArkDock 端到端通过。

下一步：按 [集成说明](native-preview/integration.md) 审查潜在共享文件，前端
先保存自己的修改，再在集成分支合并并运行实际桌面与二进制帧门禁。
本会话保留独立分支、工作树和证据；没有合并、推送或发布。
