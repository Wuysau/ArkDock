# 原生后端实施计划

设计：[design.md](design.md)。仅写本工作树，保留原工作树所有未提交修改。

1. **隔离与基线**：核查 Git/Harness/已有接口；从已提交共同基线建立工作树。
   运行 `cargo test --locked --manifest-path src-tauri/Cargo.toml`。
2. **官方研究与工具实验**：读取固定提交的 Previewer/Ace/编译前端源码；
   形成 `official-architecture.md`、`protocol.md`。核验本机 SDK 元数据、摘要和
   `PreviewBuild` 实际参数/输出；日志存 `artifacts/native-preview/`。
3. **二进制帧**：新增 `preview/frames.rs`，提供类型化元数据、严格 JPEG40
   解析、单帧有界邮箱、顺序/会话/代次拒绝和计量；异常头、长度、图像尺寸、
   旧帧与关闭测试。与现有后端兼容，生成协议仍由 `protocol/model.json` 管理。
4. **构建/会话**：在 `preview/windows.rs` 使用已核验预览专用任务和产物；
   实际验证重载后选择刷新策略，不以不可靠帧归属推断成功。保持串行、取消、
   信任、构建失败旧帧和 Job 清理；新增参数/恢复回归。
5. **诊断与独立验收**：新增只读 `preview/environment.rs` 和独立 Rust 驱动，
   必要依赖分别报告 AVAILABLE/MISSING/INCOMPATIBLE/UNVERIFIED/ERROR。
   实际验收程序走正式后端，执行 ArkTS 首帧、点击、源码更新、失败恢复和清理。
6. **契约与交付**：新增 `frontend-contract.md`、`windows-setup.md`、
   `validation.md`、`integration.md`。运行格式、clippy、Rust/便携契约测试，
   独立审查并修复；记录 PASS/FAIL/BLOCKED/SKIPPED。保留分支供安全集成。

## 审查重点

- 验证配置存在与实际运行成功不能混为一谈；未知 SDK/架构/WSL 必须阻塞。
- 畸形帧/错误尺寸/超额分包不得分配无界缓存；不能给旧帧套新 build 身份。
- 编译中新增写入、删除/改名仍有下一代，错误时不得宣称最新画面。
- 关键输入顺序、边缘坐标和停止后的迟到请求拒绝。
- 只清理自有进程与资源，不连接或操纵另一工作树开发会话。

## 进度

- 隔离完成：`feat/native-preview-backend`，基线 `aab0f5e`。
- 基线 Rust 测试 PASS：61 lib + 2 CLI + 1 OS watcher + 6 workspace，退出 0。
- 官方固定提交研究、SDK 参数核验、预览专用构建、二进制接口、独立驱动与文档完成。
- 最终独立 native run-6 退出 0：T1–T7 PASS，真实 Count:1/SECOND 图片人工核验。
- 原生点击/源码标签区域断言已加强，回执 SGR/退出竞态、信任失效取消与传输生命周期回归通过。
- Rust 111 项、Node 31 项、fmt/clippy/TS/构建通过；基础聚合仍有原提交 Harness 和 UI lint 两项 FAIL。
- 自动 SDK 环境变量探测只读实测通过，原工作树没有写入。
- 下一步由集成会话审查共享文件，合并后执行 Tauri/WebView2 二进制帧及完整桌面门禁。
