# ArkDock 前端与动效增量计划

> 执行：当前会话实施，独立状态/帧和后端审查并行；完成后独立代码审查。

目标：将确认的右侧窄窗与可分支交互落实到现有 React/Tauri 工程。
规格：[界面](../../design/arkdock-ui.md)、[动效](../../design/arkdock-motion.md)。
约束：保留现有工程与用户修改，无 Git 元数据，不初始化仓库；不替换状态管理或加入动效依赖。
修改前副本：artifacts/ui-2026-10-09/before；真实验证记录存同一任务证据目录。

## 工作项

- [x] 核验入口/架构/现有 IPC/两图；保存原参考与设计约束。
- [x] 独立 UI 维度、能力、帧背压与 V2/V3 状态测试：src/view-state.ts、src/frame-store.ts。
- [x] 窄窗 token/外壳/工具栏/面板/诊断/减少动画/焦点；App 与 PreviewStage 消费状态适配。
- [x] 最小原生窗口动作与尺寸；保留 scope/window 校验、原生拖动和独立信任。
- [x] 独立开发入口、可控 fixtures、V1–V4 分支与动态检查；正式产物排除演示。
- [x] V5 实际 Windows/SDK 门禁、独立审查；通过与失败子范围分别记录。

## 接口与风险

输入：AppSnapshot、NativeFrame、CommandArgs；适配 deriveUiState(snapshot, frame) 和能力原因。
后端只有点击、默认 Index/360×640 原生 Previewer；不可提前承诺旋转、滑动、文字、页面/设备切换。
审查重点：旧响应跨目标、停止后输入、首帧迟到、模态焦点/IME、缩放后坐标、候选全量与弱候选。
SDK native-counter 是工具自有验收工程；不能执行其他未信任工程。

## 验证与下一步

执行 pnpm test:portable、lint/typecheck/build、verify；单独执行前端 fixtures 截图/交互与原生门禁。
分别记录退出码与 PASS/FAIL/BLOCKED/SKIPPED。浏览器尺寸/DPR 不证明多 DPI Windows。
当前下一步：实现状态/帧与窄窗组件；原生缺口由只读审查确认后最小补充。

## 2026-10-09 检查点

基础 verify 初轮退出 0；Rust desktop 71 项退出 0；新增审查修复后便携 30 项退出 0。
独立审查修复：首帧 CSS 保持连续、能力面板订阅实际帧、跨面板焦点恢复、旧过渡中断、
旧响应/导航代次、同目标生命周期取消重试、完整空格路径脱敏、模态外提示不可聚焦、
真实 dockHint 分类。完整最终验收尚在运行，先前失败保留独立报告。
用户指示先保存代码检查点：aab0f5e。已初始化本地 Git，后续代码修复有实际 diff；
设计/图片/Harness/验收产物不纳入提交。原始无 Git 记录只作为本任务开始时的环境事实。
当前下一步：完成浏览器与 Windows/SDK 真实驱动，检查证据和退出码，更新逐 ID 结论。

## 前端检查点完成

代码提交 c090418。最终 22/22 浏览器检查 PASS、93 张截图；31 便携回归 PASS。
真实无边框 WebView 八组、窗口关闭与当前 SDK 七组 PASS。
实际欢迎截图 artifacts/ui-2026-10-09/native/ui-welcome-window.png：440×820 DIP、192 DPI。
Windows 最大化并排/迟到恢复压力 FAIL 保留，未删除断言或放宽生命周期检查。
用户最新要求零间距、共同缩放和共同最大化，转入 [共同布局计划](joint-layout.md)。
