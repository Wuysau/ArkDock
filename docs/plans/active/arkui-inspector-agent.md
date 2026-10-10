# ArkUI 检查与选择上下文实施计划

> 执行：主会话集成，三个独立模块并行实现；最终统一验证与审查。

**目标：** 按用户提供的完整规格，在真实 JPEG 帧上建立稳定 Canvas、互斥检查模式和受身份约束的选择交付。

**基线：** `dev@db2bd49`，独立 `feat/arkui-inspector-agent-bridge` worktree。
原 `feat/joint-dock-layout` 的未提交审计修复不包含在本基线，不覆盖、不静默搬运。

**结构：** `src/preview` 拥有二进制解码、显示与有界消费；`src/inspector` 拥有模式、覆盖层和面板；Rust Inspector 拥有受控原生查询，selection 拥有截图及过期上下文，agent 拥有会话能力和交付状态。主入口只接线，IPC 仍由 `protocol/model.json` 生成。

## 约束与审查重点

- 仅真实 Previewer 像素；商业 SDK 能力必须单独实测。
- 旧解码、旧组件树、旧选择与旧消费者不得跨会话或构建生效。
- 先成功协商二进制，再消费；同会话消费者替换由 consumerId 防止迟到关闭。
- Inspector 无真实树时明确区域选择；源码行号无证据不得标 Verified。
- Agent 无当前线程写入通道时 UNSUPPORTED，不启动替代线程、不读聊天库。
- 缩放、DPI、隐藏恢复和连续点击必须有适用验证；不能用便携单测证明桌面原生行为。

## 任务

- [x] 1. 诊断基线：核对帧转换与输入路径，实测 SDK Inspector 和当前 Agent 接口；保存日志、版本、耗时。无法取得的数据记录 BLOCKED。
- [x] 2. 二进制显示：严格封包验证、有限轮询、解码完成后原子绘制、最后有效帧重绘与回收；测试异步乱序、过期帧、协商失败和消费者替换。
- [x] 3. Inspector：受控查询与树模型；透明覆盖层本地命中或区域框选，Esc 退出且选择点击不传入 ArkUI；测试层级、坐标和过期响应。
- [x] 4. 选择交付：真实显示帧关联、截图裁剪、TTL 本地存储、只读 MCP、可验证绑定与状态机；测试错工作区、会话切换、过期、重复发送和伪回执。
- [x] 5. 集成：生成共享类型、main/origin/scope 校验、工具栏左侧入口、详情和意见面板、复制/导出与真实能力说明。
- [x] 6. 验证：typecheck/lint/ipc:check/test:portable/verify、Rust desktop 测试、真实 WebView2/SDK 回归；更新状态、性能与协议证据、集成冲突说明。

## 验证记录

代码已集成。SDK Inspector 和真实 GUI 六组 PASS；原生 verify:native 八组 PASS；便携 59/59 PASS。性能实测 p50/p95 180.7/305.2 → 140.0/207.3 ms。最终高频画面、DPI 缩放、原生最小化暂停/恢复及共同布局 11 组 PASS。收尾保留独立分支，完整报告位于 docs/evidence/inspector-agent-selection-2026-10-10.md。当前 Agent 通道 UNSUPPORTED，双屏 BLOCKED，未合并悬浮球及发布 SKIPPED。
