# Inspector、选择交付与原生帧优化验收

环境：Windows 11，Node 24.21.0、pnpm 11.22.0、锁定 Rust/Tauri 依赖；Command Line Tools
26.0.0.851，SDK 26.0.0.105/API 26。独立分支 `feat/arkui-inspector-agent-bridge`，
基线 `dev@db2bd49`。未覆盖原工作树未提交修改、未合并 dev、未推送或发布。

## 用户要求的十二项交付事实

1. **根因。** 最小化时 WebView2 的 document.hidden 不反映原生状态，已接入自有 HWND 可见性门禁。兼容链路在受限 JPEG 验证后重复解码 JPEG、转 PNG/Base64，再由浏览器解码。
   基线 12 次普通点击没有设备框重新挂载，也没有证据证明 first-frame 动画重播。
   新消费者的 WebView2 timer receiver 错误已实际复现并修复；唤醒异常不会再留下输入锁。
2. **真实测量。** 12 次真实 ArkUI 点击，反馈 p50/p95 从 180.7/305.2 降至 140.0/207.3 ms。
   指标为点击至浏览器解码后的 rAF，不能等同显示器发光时刻。JPEG 浏览器解码等待
   1.7/2.8 ms，并未比基线 PNG 的 0.8/1.6 ms 更快。细分、CPU task time、JS heap 和
   丢弃计数见 [性能证据](preview-performance.md)。原生总 CPU/内存、SDK 内部 render 未测。
3. **Inspector 协议。** 实际 SDK action `inspector`、`inspectorDefault`、响应版本 1.0.1 和
   JSON 字符串树均 PASS；5 个节点，点击后的 Count:0→1 与真实 JPEG 对应。
   有界管道读取消除了多次 16 ms tick 等待：同场景 12 查询 p50/p95 101/101→34/50 ms。
   这是查询时间，不能冒充输入反馈时间。见 [协议实测](../native-preview/inspector-validation.md)。
4. **两种模式。** 默认 interact；左侧箭头切 inspect，透明独立覆盖层高亮、固定选择，
   Esc 或再次点击退出。真实 GUI 验证选择不触发原生 onClick，退出后原生点击恢复，
   模式切换的 session/build/generation 保持一致。
5. **原生信息。** 可读取类型、ID、父子关系、深度、原生矩形、响应给出的可见性、属性、
   `$debugLine`/`$viewID`。实测 Button 矩形 27,333,306,56；JPEG 360×640/19100 字节，
   裁剪 306×56/4494 字节。截图、裁剪、stdio MCP 的 SHA256 一致。
6. **源码和采样限制。** 本样例 Index.ets:21 的原生调试提示为 Candidate；尚未核验当前
   文件与构建源码一致性，所以不标 Verified。路径相对鸿蒙应用根，Agent 须确认包/模块与
   实际源码位置。页面名、可靠 paint order、原子 tree/frame 标记缺失；复杂叠放/滚动未验证。
   新图像改变 JPEG 内容锚点后拒绝旧命中，无法保证一致性时明确区域选择。
7. **当前会话发送核验。** Codex app-server 公开 thread/turn 方法不证明外部程序拥有本机
   Desktop 当前线程写权限；Claude Channels 需要显式启用的会话接入。本环境均没有经核验
   的活动会话写入通道，适配器 UNSUPPORTED；未读取聊天数据库或凭据。
8. **真正完成的发送适配器。** 生产适配器 0 个，真实消息提交 0 次。没有新建会话、第二
   个 Agent 进程、键盘注入或剪贴板冒充 SUBMITTED；按钮根据真实能力保持禁用。
9. **接口和降级。** 已实现绑定、能力、准备/发送/状态、scope 校验、需求指纹去重与回执
   状态机；Codex/Claude transport 待宿主授权接入。可保存意见、生成完整提示词、读取/导出
   原始 JPEG 与裁剪，MCP 仅显式 grant 读取；真实 MCP 与越权拒绝已测。见
   [交付设计](../design/agent-selection-delivery.md)。系统剪贴板和导出保存对话框本轮 SKIPPED。
10. **Windows GUI。** `tools/inspector-ui-smoke.mjs` 退出 0，六组 PASS，包含组件选择、图片
    身份、错误 scope/build、MCP、缩放命中、区域选择与 Esc。`pnpm verify:native` 退出 0，
    八组实际断言 PASS：真实构建/Canvas、信任取消恢复、点击、独占帧流、源码更新、编译失败
    旧画面及恢复、停止自有 Previewer。真实 50 ms 定时 ArkUI 更新、155 帧、DPI 缩放和最小化/恢复 PASS；最小化时原生 IsIconic=true、document.hidden=false，稳定观察期绘制 0 帧。共同布局另行记录。
11. **证据及未测范围。** 基础 verify、便携回归、desktop,native-tools all-targets 已执行；
    最终命令/退出码以本报告后续表和 artifacts 为准。双屏跨 DPI BLOCKED，悬浮球分支未合并
    故共同回归 SKIPPED；发布和安装器 SKIPPED。中间 SDK 空缓存、窗口放置、驱动条件、
    Windows 文件锁及存储虚拟化失败均保留，没有删掉真实断言以通过。
12. **集成冲突。** 主要接线在 src/App.tsx、src-tauri/src/desktop.rs、protocol/model.json
    及 src-tauri/tauri.conf.json；两端生成文件须合并 model 后重新生成。src/styles.css、
    src/motion.ts、windowing 模块未改。Inspector/Canvas/CSS/selection/agent 独立文件可先
    集成；consumerId 每次协商唯一且贯穿 enable/read/disable，旧消费者不得关闭新流。
    悬浮球隐藏需要接入原生窗口可见性门禁。原工作树未提交 Todo/窗口审计修复须另行语义合并。

## 可复核的本地证据

- `artifacts/preview-performance-baseline.json` / `preview-performance-optimized.json`：真实 12 次点击。
- `artifacts/native-preview/inspector-final-20261010/`：SDK/树/截图/输入/查询/构建/运行日志。
- `artifacts/inspector-ui-final/report.json`：六组 GUI + 实际 stdio MCP；PNG、原始 JPEG、裁剪。
- `artifacts/native-gate.json` / `native-smoke.json`：本轮完整原生门禁 PASS。
- `.local/inspector-verify-final.log` / `artifacts/verify.json`：基础环境、编译和契约门禁。
- `.local/inspector-desktop-tests-final.log`：Windows desktop,native-tools 全目标测试。

选择存储绑定受控 Tauri AppData 固定子目录，拒绝 reparse/symlink，逐次校验 scope、大小和
摘要。Windows 打包宿主重定向该子目录曾使严格 parent 比较拒绝合法存储；同机诊断修复前
退出 1，修复后退出 0，随后原始图片、保存意见及实际 MCP 完整通过。十分钟过期清理与
十秒运行时 lease 防止旧工程/构建冒充当前数据。

## 最终命令与退出码

| 命令 | 退出码 | 结果 / 证据 |
| --- | ---: | --- |
| `pnpm install --frozen-lockfile` | 0 | 锁定依赖及供应链检查完成，仅此 worktree |
| `pnpm typecheck` / `pnpm lint` / `pnpm ipc:check` | 0 | 基础 verify 同时覆盖 |
| `pnpm test:portable` | 0 | 59 PASS，包含异步解码、visibility、同会话旧 lease、MCP scope |
| `pnpm verify` | 0 | 15 项基础门禁 PASS，`artifacts/verify.json` |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop,native-tools --all-targets` | 0 | 140 PASS，`.local/inspector-desktop-tests-final.log` |
| `cargo build --locked --manifest-path src-tauri/Cargo.toml --features desktop,native-tools` | 0 | 当前 Windows debug exe；不是发布安装包 |
| `pnpm verify:native` | 0 | 最终 PID 54208，8 组原生断言，`artifacts/native-gate.json` |
| `node tools/inspector-ui-smoke.mjs` | 0 | 最终同 PID，6 组 GUI/MCP，`artifacts/inspector-ui-final/report.json` |
| `node tools/performance-probe.mjs optimized` | 0 | 最终 12 次点击，140.0/207.3 ms，`artifacts/preview-performance-optimized.json` |
| `node tools/performance-probe.mjs stress` | 0 | 155 帧/11.508 s，缩放及实际最小化暂停/恢复，`artifacts/preview-performance-stress.json` |
| `pnpm verify:joint` | 0 | 11 组贴边、最大化、恢复、联动缩放及独立窗口，`artifacts/inspector-joint-regression/joint-smoke.json` |

原生驱动显式指定 `ARKDOCK_TEST_PID`/`ARKDOCK_PROBE_PID` 与端口 9447，并核验
WebView2 调试服务为该进程后代；只启动/清理自有 fixture。另一个磁吸分支实例未操作。
并行桌面放置会干扰初始鼠标前置；稳定几何、等待已接受 placement 补偿及独立进程命名
后才得到成功回执，相关 FAIL 原件留在 `.local`/各验收目录。SDK abort 产生的空缓存只在
本次自有 fixture 中隔离，未清理其他工程或全局缓存。

收尾保留独立分支/worktree，下一步是用户安排集成后复跑悬浮球 hide/show 与双屏 DPI；
当前环境不存在可验证的当前 Agent 写入通道，因此下一适配器需宿主明确注册和真实回执。
