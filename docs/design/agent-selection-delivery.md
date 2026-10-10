# 选择上下文与当前 Agent 发送

核查与实现日期：2026-10-10。当前实现保留真实预览截图、区域裁剪、组件证据与用户要求；Codex / Claude 当前会话直接发送均为 `UNSUPPORTED`。没有生产适配器返回 `SUBMITTED`。单测中的假适配器只验证路由状态，不能作为宿主发送证据。

## 选择数据与权限

`selection_create` 使用后端保留的已消费原生 JPEG，严格比较完整 `BinaryFrameMetadata`：workspace/session/build/generation/sequence、尺寸、格式、长度与协议版本。前端不能提供任意截图字节、组件节点、源码证据或文件路径。创建时要求当前可信、运行中、非过期预览。静态场景可以不产生新帧，因此使用运行时与构建身份、后端保留的画面锚点确认当前性，不按图片接收时间任意失效；异常未来时间戳拒绝。

后端在 Tauri app data 的 `selection-context-v1/sel-…/` 保存 `context.json`、原始 `screenshot.jpg`、真实 JPEG 解码后裁剪的 `crop.jpg` 与人类可读 `prompt.txt`。绝对路径不放入 IPC 选择结果。普通状态快照不包含图片。每张图片有独立尺寸、长度和 SHA-256。读取前复核工作区、会话、构建与过期，再核验图片摘要。

原生组件的负坐标或超出画面的真实矩形保留在 context；裁剪图片仅取其与原生表面的有效交集。没有可见交集的组件拒绝保存。用户区域选择必须完全位于实际画面内。

选择有效期为 10 分钟，最多保留 32 份有效上下文。创建/启动/显式清理会清除过期数据；清理只处理自有目录中固定文件名，不递归删除、不跟随符号链接、不读取项目无关文件。换工程、换会话、重建、失去信任或停止后不能通过产品 IPC 读取或发送旧选择。用户要求由 `selection_update_instruction` 关联已有 selectionId，发送准备之后再改要求会使旧请求失效。

真实 Windows GUI 首轮暴露了 app data 虚拟化：受控逻辑 app-data 父目录规范化到用户 Roaming，而新建的固定选择子目录规范化到宿主应用包的物理缓存，原先按父路径字符串比较错误拒绝为 `SELECTION_DIRECTORY_UNSAFE`。修复保留固定逻辑子目录入口并拒绝全部 Windows reparse points（包括 junction），随后将所有读写绑定该子目录的实际规范化根。没有任选目录兜底；其他平台仍校验父路径。`cargo run --locked --manifest-path src-tauri/Cargo.toml --example selection-store-probe` 在同一真实 app data 修复前退出 1，修复后退出 0。诊断仅处理 ArkDock 自有选择子目录，不读取偏好或聊天数据；完整宿主包路径和 SID 不保存在本文。

`selectedNode` 只能来自后端原生 Inspector 缓存及同运行时/构建/查询版本核验。SDK 26 Inspector 不返回原子帧标识：`inspectionExactFrameMatch=false` 与 `inspectionReason` 明确保留此限制，不能把主机查询前后序列观察说成原生同帧证明。组件树不可用时保存 `region` 选择。

源码证据不会根据截图或组件类型猜行号。原生 `$debugLine` 中形如 `entry/src/main/ets/pages/Index.ets(12:7)` 的受限相对路径可保存为 `candidate`；绝对路径、穿越段、控制字符和无效行列拒绝。当前代码没有证明源码自构建起未变化、行上组件匹配，因此没有 `verified` 映射。

## 当前会话绑定与状态

`AgentRouter` 保存 agentKind、当前 ArkDock 目标 token、PID + 进程启动身份、workspaceId、Agent session/thread ID、transport/capabilities 与 60 秒过期时间。窗口标题、产品名、PID 或用户填写的会话 ID 均不证明活动线程写入权。显式更换同 Agent/目标绑定会撤销之前绑定；进程重启、目标变更、工作区错配与过期被拒绝。

当前两种宿主的能力全部由适配器状态返回为不可用。手动绑定总是 `writable=false`；不能通过前端布尔值打开发送。发送状态具有 `READY`、`UNSUPPORTED`、`SESSION_UNVERIFIED`、`SENDING`、`SUBMITTED`、`FAILED`、`EXPIRED`。准备会生成可复制的提示词。发送前再次读取选择、图片并复核精确目标绑定；改变要求、线程、进程或工作区使请求拒绝。去重请求由完整选择 + 用户要求 + 目标会话产生；重复点击返回同一请求。只有匹配请求和精确 thread 的非空可靠提交回执才能表示 `SUBMITTED`，此状态也不代表源码修改完成。

代码没有生产直写通道。没有启第二个 Codex/Claude、恢复同一会话制造并发写入、读取私有数据库/聊天/凭据、注入 UI 或模拟键盘。复制、导出和 MCP 读取不会更新为 `SUBMITTED`。

## 官方接口核查

| 来源 | 已知能力 | 本次结论 |
| --- | --- | --- |
| [Codex App Server](https://learn.chatgpt.com/docs/app-server) | `thread/list`、`thread/read`、`thread/resume`；`turn/start` 接受文本/图片/本地图片；`turn/steer` 要求匹配 `expectedTurnId`；提交接受与任务完成是不同事件 | 协议可供自有客户端使用。没有核验 ArkDock 能连接用户当前 Codex Desktop 的同一授权写入通道，也没有证明本次活动线程身份；默认 `UNSUPPORTED` |
| [Claude Channels](https://code.claude.com/docs/en/channels) 与 [reference](https://code.claude.com/docs/en/channels-reference) | 已显式启用并允许的 channel MCP 可向现有运行中 CLI 会话推送事件；普通 MCP 仅查询，不主动推送 | 这是可能的后续合法集成路径，不能说 Claude 从不支持。当前未实现自有 channel 插件、活跃会话绑定、配对、图片授权和可靠提交回执，`UNSUPPORTED` |
| [Claude Remote Control](https://code.claude.com/docs/en/remote-control) 与 [CLI reference](https://code.claude.com/docs/en/cli-reference) | Remote Control 同步到受支持的 Claude 网页/移动端；resume 是恢复对话 | 这些文档不能证明任意 ArkDock 程序获得当前活跃会话外部写入权；没有启动或恢复第二进程 |

未来生产适配器必须通过明确授权的通道注册，证明当前线程/工程/目标生命周期并在提交前再次核验。宿主忙碌时，只有已验证 steer 或明确队列能力才能使用；不能默认打断。需要可复现的真实提交回执后才可以开放主要发送操作。当前单测专用 transport 与 receipt 接口不编译进生产提交路径。

## 显式只读 MCP

`tools/selection-provider.mjs` 是无依赖、stdio JSON-RPC MCP server，仅提供：

- `arkdock.get_current_selection`
- `arkdock.get_selection`
- `arkdock.get_selection_screenshot`（原图或 crop）
- `arkdock.get_selection_context`

stdio 与工具返回依据 [MCP 2025-06-18 传输](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)、[生命周期协商](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle) 和 [tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)。未知的新协议版本协商至明确支持的版本，不宣称已实现该新版本；`notifications/initialized` 之前拒绝工具调用。

每个调用检查显式 grant、工具权限、selectionId 白名单、workspace/session、TTL、原生构建身份和图片摘要。它不发送通知、不实现 Claude channel、不修改工程或 Agent 配置。不向 Agent 回传本地绝对路径；组件属性文本被注明为不可信技术数据。

ArkDock 发布 `runtime.json` 10 秒租约：记录当前 workspace/session/build/generation 与是否可用。主机状态变化必须更新，运行中周期续期。MCP 在每次读取时复核租约，重建/停止或租约过期立即拒绝旧选择。只有文件中有 saved context 而没有有效 live lease 时明确 `BLOCKED`。

用户选择并保存明确要求后，可以自行配置一个只授予特定选择的 grant 文件。示例中的路径、工作区、会话、selectionId 和到期时间必须来自用户当前确认的选择，下面是配置结构示例，不能直接当作有效授权：

```json
{
  "version": 1,
  "directory": "C:/Users/USER/AppData/Roaming/dev.arkdock.desktop/selection-context-v1",
  "workspaceId": "CURRENT_WORKSPACE_ID",
  "sessionId": "CURRENT_PREVIEW_SESSION_ID",
  "selectionIds": ["sel-CURRENT_SELECTION_ID"],
  "allowedTools": ["arkdock.get_selection", "arkdock.get_selection_screenshot", "arkdock.get_selection_context"],
  "expiresAt": 0
}
```

在 Agent 自己支持的配置界面中手动添加 stdio server：可执行文件 `node`，参数按数组分别填写 `tools/selection-provider.mjs` 的绝对路径、`--directory` 与目录、`--workspace` 与工作区 ID、`--session` 与预览会话 ID、`--capability` 与 grant 的绝对路径。只有两个路径都为绝对路径且 grant 尚未到期才启动。ArkDock 不自动改全局 MCP 或 Agent 配置；grant 文件由用户明确配置，不是工作区发现证据。

工具输出是选择上下文读取回执，不是 Agent 消息提交回执。提示词复制不会自动附带图片；需要用户导出截图或让已经授权的 MCP 客户端读取图片。

## 本模块验证记录

环境：Windows，独立 `feat/arkui-inspector-agent-bridge` worktree，Rust 锁定依赖；2026-10-10。以下是合成单测，不能证明 Windows GUI、真实 ArkUI截图或生产 Agent 提交。

| 命令 | 退出码/结果 | 覆盖 |
| --- | --- | --- |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib selection::` | 初轮 0，5 PASS；新增静态画面回归由整体验证覆盖 | 工作区/会话/构建/TTL、非法 bounds、不伪造源码映射、真实 JPEG 解码/裁剪流程的合成图、持久化/修改要求/重启、图片篡改；新增旧接收时间但当前构建仍可选择、异常未来时间拒绝 |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib agent::` | 0，5 PASS | 不授予假写入权、错误工作区/目标进程、线程切换/过期、重复请求、严格回执和需求改变 |
| `node --test tests/portable/selection-provider.test.mjs` | 0，8 PASS | 显式 grant/只读工具、工作区/会话/TTL、篡改/额外参数、重建/停止/live lease、路径穿越/符号链接、stdio 初始化/限额、较新客户端的版本协商与初始化顺序 |

生产 Codex/Claude 一键发送：`UNSUPPORTED`。未来适配器的真实目标线程、图片、忙碌行为与回执验收：`BLOCKED`，原因是本次没有已注册并核验的活动会话写入通道。该限制不会阻止选择、修改要求保存、复制、截图读取和明确授权的 MCP 底座。

## 真实 Windows GUI 与 MCP 回执

2026-10-10，商业 SDK 26.0.0.105、实际 `examples/native-counter`、当前分支独立任务进程、WebView2；`node tools/inspector-ui-smoke.mjs` 最终退出 0，六组 `PASS`。在工具栏四列布局、原生窗口可见性门禁及传输租约代次修复后的最终源码再次执行六组，2026-10-10 16:46（Asia/Shanghai；回执 UTC `08:46:44.124Z`）仍全部 `PASS`。驱动通过 TCP 调试监听进程 ancestry 校验只连接明确的任务 PID，没有采集整个桌面，也没有发送任何 Agent 消息。

- 原生组件树返回 5 节点；悬停真实 `Button` 高亮，点击固定组件，实际日志的 native count 不增加，也没有 `click_preview` 请求。
- 实际 Button 矩形 `27,333,306,56`，父节点 5、深度 1；原生 `$debugLine` 给出 `entry/src/main/ets/pages/Index.ets:21`，保存为 `CANDIDATE`。组件树与截图仍注明非原子采样。
- 截图为真实原始 JPEG；裁剪为真实 JPEG 区域；经 Tauri 二进制 IPC 读取后 SHA-256、字节长度、解码尺寸都与 `SelectionContext` 相符。
- 实际 textarea 输入并点击“保存选择上下文”；提示词包含同一 selectionId 与保存的用户要求。故意错误构建/会话被拒绝；两种 Agent 的能力均 `UNSUPPORTED`，主要发送按钮禁用，回执为空。
- 为同一已确认选择创建仅 60 秒、单 selectionId 的本次测试 grant；实际 stdio MCP 初始化、上下文读取、原图与裁剪读取均通过，图片摘要与原生 IPC 一致，未授权 selectionId 被拒绝。没有配置任何 Agent/global MCP，没有主动推送消息。
- 原生窗口缩放后关闭浮动详情，重新命中真实 Button；手动区域模式保存真实裁剪；Esc 退出后真实 ArkUI 点击恢复，native count 增加 1，session/build/generation 未替换。

证据：`artifacts/inspector-ui-final/report.json`、`component-selection.png`、`resized-hit.png`、`normal-click-restored.png`、`native-preview.jpg`、`crop.jpg`。这些当前文件对应上述最终源码重复验收；前一版真实 PASS 和图片按 `previous-source-*` 保留。截图中的累计诊断数量包含此前真实原生门禁故意触发的失败案例，当前状态仍为有效原生预览，不清除历史错误美化截图。浮动详情覆盖 Canvas 的下部，重新选组件前可用详情上的关闭按钮；缩放测试遵循这一真实 UI 操作，没有把点击浮层后的无响应判为坐标映射失败。剪贴板和原生导出文件对话框本次未操作，核心提示词和图片 IPC 已验证。

此前失败分别保留：`artifacts/inspector-ui/store-open-failure-report.json`（实际存储虚拟化拒绝，已修复）、`artifacts/inspector-ui-final/bootstrap-race-failure-report.json`（驱动读取异步选择前状态，已改为等待真实关联）、`panel-occlusion-partial-report.json`（四组真实 PASS 后，驱动点击详情浮层覆盖区域，已增加关闭详情操作）。另一次明确任务进程退出后，调试所有权检查返回 `BLOCKED`；退出原因未证实，未触碰其他分支的应用。最终使用任务专属进程名称和独立 WebView profile 完成上述实际验收；这些历史失败没有冒充成功。
