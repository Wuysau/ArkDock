# Codex transports

运行脚本需要 Windows 和 Node >= 24.18。受管理 App Server 协议当前固定为 Codex CLI 0.159.2。

| 路径 | 实现 | 验证边界 |
| --- | --- | --- |
| ArkDock 管理的 App Server | 自有 stdio 子进程、新线程、文本及授权 localImage、线程核验、回执 | 已实测 Codex CLI 0.159.2 的 Windows 握手、线程读取、真实 Inspector 选择经生产 Bridge 的图片提交；模型完成被上游断流阻塞 |
| 独立启动的 CLI | 不接管现有终端会话 | 使用用户明确选择的 [Windows 辅助填写](assisted.md) |
| Desktop 可选插件 | 受宿主约束的 thread entrypoint、MCP 工具及 ui/message | 协议夹具已验证；尚无已验收的真实宿主版本，启用与验收前 BLOCKED |

## 管理的新会话

在 ArkDock 对当前可信 Windows 工作区创建 Codex 配对，将返回的 JSON 保存至用户选定的本地文件。文件包含 bearer token，应只留本机，不放进提示词、命令行参数或版本库。

```powershell
node tools/agent/codex-app-server.mjs --create-managed `
  --pairing-file 'C:/absolute/arkdock-pairing.json' `
  --codex-executable 'C:/absolute/codex.exe'
```

`--create-managed` 明确创建一条新对话。需要该 Agent 修改工程时额外传 `--workspace-write`；默认只读。可用 `--model <id>` 显式选择用户账户支持的模型，省略则保留宿主配置。脚本使用绝对 exe 路径与结构化参数，不调用 Shell、npm 启动包装器、私有数据库或已运行的 daemon。

ArkDock 中选择实际登记的受管理会话后才可发送。登记的 sessionId 来自 `thread.sessionId`，conversationId 来自 `thread.id`，instanceId 标识本次自有连接。发现进程或手工输入线程 ID 均不提供这种授权。脚本只允许已核验的 0.159.2；更换版本需重新生成 schema 和实测。

每次 poll 和发送前调用 `thread/read`，核对线程、会话树、工作区及状态。忙碌时只续 lease；默认不取出待发请求。底层实现 `turn/steer`，但仅接受明确选择且与已观测 active turn 一致的 expectedTurnId，正常 UI 不自动 steering。没有恢复历史线程的入口。`thread/resume` 是公开历史恢复 API，本身不能证明正在运行的 CLI 或 Desktop 会话所有权。[App Server 协议](https://learn.chatgpt.com/docs/app-server)

发送使用已有 SelectionProvider 的动态 grant；deliveryId 前缀、grantId、选择范围、上下文摘要、运行时 lease、图片大小及摘要均须有效。只有 ArkDock 存储的已授权 JPEG 能成为 `localImage`。项目切换、授权替换或图片改动会阻止发送。

回执定义：

- `TRANSPORT_ACCEPTED`：对应 `turn/start` / `turn/steer` 响应返回有效 turn ID。
- `SUBMITTED`：同一 thread/turn 的 userMessage 事件携带匹配的 `clientId`，证明提交到该线程。不能把服务端随机生成的 item.id 当作 clientUserMessageId。
- `AGENT_ACKNOWLEDGED`：管理的 App Server 适配器不推断这个状态；userMessage 事件不能证明模型实际消费请求。
- `COMPLETED`：上述消息已提交，且匹配的 `turn/completed` 明确返回 completed；不代表工程测试自动通过。
- `FAILED` / `UNKNOWN`：明确拒绝或失败 / 已发出后的超时、断开及无法确认结果。未知请求不自动重发。

服务端审批请求会被拒绝，其他需要宿主交互的请求明确返回不支持。只有本脚本创建的子进程会被关闭。断线或 Bridge 回执失败后停止接收新请求，恢复需要新配对。

## Desktop 可选插件

插件仅包含会话桥面板；完整鸿蒙预览仍在独立 ArkDock 窗口。公开扩展为宿主内 MCP App 提供 `ui/message`，可向当前对话发送或填入草稿；它不是独立进程可调用的桌面 REST API。thread entrypoint 的 UI 实例由宿主按对话隔离。[官方扩展规范](https://github.com/openai/mcp-extensions/blob/main/docs/spec.md#uimessage-extensions)

生成待审配置，不安装、不改全局配置：

```powershell
node tools/agent/codex-desktop-plugin.mjs `
  --write-config 'D:/absolute/ArkDock/artifacts/agent-delivery/desktop-config' `
  --pairing-file 'C:/absolute/arkdock-desktop-pairing.json'
```

输出目录必须是当前仓库 `artifacts/agent-delivery` 下尚不存在的新目录。生成 `plugin.json`、包含实际 Node/脚本绝对路径的 `mcp.json`、待合并的 `marketplace.json`。这三个文件不含 token。仓库内还提供 [插件清单](../../plugins/arkdock-session-bridge/plugin.json) 和 [MCP 配置模板](../../plugins/arkdock-session-bridge/mcp.json.example)。

启用步骤必须在用户明确授权后执行：

1. 在要使用的仓库，将生成的 marketplace 条目合并到 `.agents/plugins/marketplace.json`。不得覆盖已有条目；source.path 相对于仓库根。
2. 按宿主的本地插件流程刷新/重启，选择 `ArkDock local bridge`，安装并启用 `ArkDock Session Bridge`。宿主可能将安装结果存入用户插件缓存；这是独立的宿主授权操作。
3. 在 ArkDock 创建新鲜 Codex 配对并保存到配置指定的 pairing-file；若插件已运行，重启该 MCP 连接加载新配对。
4. 在目标对话打开 `ArkDock Session Bridge`，点击 `Connect this conversation`。连接成功后，再从 ArkDock 登记列表明确选择该会话。
5. 先用公开测试工程中的无私有内容选择验证一次真实消息与显式回执，再扩展到实际工作区。

本地 marketplace、portable plugin.json/mcp.json 及用户安装行为遵循[官方插件打包说明](https://developers.openai.com/plugins/build/plugins)。首次安装与现有对话能否重新加载插件仍须由本机宿主验证，不能保证安装后无需新对话。

连接要求同时满足：

- 宿主 UI 协商 `2026-01-26`、serverTools、文本消息及 `experimental["openai/message"]` 能力；握手字段遵循 [MCP Apps 协议](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx)。
- MCP 客户端支持 `roots/list`，且返回唯一、与配对工作区一致的本地 file root。多根、WSL、远程、缺 roots 均阻塞。
- 每个 MCP tools/call 的宿主 envelope 提供 `_meta["openai/session"]`。仅把它视为同一宿主连接中的匿名对话标识；不冒称 App Server Thread ID。[官方元数据定义](https://developers.openai.com/plugins/reference#_meta-fields-the-client-provides)
- 面板上的显式 Connect 同意与当前 UI 实例一致，且每次调用都复核根与会话。UI 不传 sessionId、workspaceRoot 或 token；工具参数拒绝伪造 `_meta`。

面板卸载、宿主上下文变化、会话不匹配、roots 变化或超过 5 秒无 UI 心跳会撤销配对。`ui/message` 成功响应只报告传输接受；Agent 必须在同一宿主会话显式调用 acknowledgment/completion 工具才能报告相应状态。读取选择工具本身不构成确认。

宿主 envelope 没有公开的签名验证机制；这里依赖明确启用的本地 MCP 宿主连接。夹具只能证明 ArkDock 不采信 UI 参数中的身份，不能证明某个实际宿主一定覆盖恶意 UI 自填的保留元数据。Desktop 验收必须确认本机宿主提供并保护该字段、准确限定 roots，且消息真实出现在已经打开的目标对话。缺少证明时保持 BLOCKED，不退回私有接口或身份猜测。

## 验证

```powershell
node --test tests/portable/codex-transport.test.mjs
codex --version
codex app-server generate-json-schema --out artifacts/codex-schema
```

portable 测试覆盖 stdio、HTTP、MCP 和 UI 协议夹具，不证明真实宿主接入。原生 Inspector 选择通过生产 Rust Bridge 提交到受管线程的链路已单独实测；上游响应流断开时保持 UNKNOWN，完整模型执行仍须真实完成事件。Desktop 插件需要用户启用后单独核验当前对话、宿主身份边界和显式 Agent 回执。
