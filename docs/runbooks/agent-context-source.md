# Agent 上下文源（MCP）接入指南

ArkDock 提供统一的本机上下文登记接口，`tools/agent/arkdock-context.mjs` 是它的 stdio MCP 适配器。
普通 MCP 客户端可通过工具显式登记原始工程和本轮实际执行目录，在 ArkDock 中确认后关联。
**普通 MCP 登记始终是建议候选**：配对、进程血缘、roots、启动目录均不能证明窗口当前显示哪个对话，
也不授予工程执行信任。本实现没有已验证的“可见对话激活”适配器，不承诺自动跟随聊天切换。

这是可选能力；不配置时，现有贴附、手动选择目录和 Codex UIA 发现保持原有行为。

## 1. 生成配对文件

1. 在 ArkDock 展开工程条，点击 **配对会话源**，选择本机凭证文件保存位置。
2. 文件含配对机密，仅供本机 MCP 配置引用；不要提交仓库、外发或把内容写入命令行参数。
3. 配对后，工程条显示配对状态及活动登记。重新生成将轮换凭证并清除旧登记；旧 MCP 进程需要重启，重新读取新文件。
4. **撤销会话源**会撤销整个配对和所有登记。仅断开当前 MCP 登记时，调用下文的 revoke 工具。

配对状态会持久化，ArkDock 正常重启后恢复，原 MCP 配置仍可使用。重连会产生新的登记身份，
因此旧的工程确认不会自动恢复，需要再次检查并选择。

## 2. 生成并加入宿主配置

需要 Node ≥24.18。下列命令仅生成示例，不安装 MCP、不修改全局配置，也不启动宿主：

```powershell
node tools/agent/arkdock-context.mjs --write-config 'D:\example\arkdock-mcp-config' --pairing-file 'C:\example\arkdock-context-pairing.json'
```

输出目录必须尚不存在。生成器会写出本机 Node 与脚本的真实绝对路径，避免宿主从其他启动目录找不到脚本：

- `claude-mcp-json.example.json`：`mcpServers` 形式的配置片段。
- `codex-config.example.toml`：`[mcp_servers.arkdock-context]` 配置片段。
- `README.md`：command/args 使用方式和 PowerShell 手动启动命令。

把相应片段合并到宿主支持的 MCP 配置位置，按该宿主说明重新加载或重启 MCP 连接。
不要假设 MCP 进程一定按聊天创建；有些宿主在整个应用或工作区内复用一条连接。
脚本移动或任务 worktree 归档前，应先更新配置中的脚本绝对路径。

## 3. 在 Agent 中连接当前工程

让 Agent 调用 `arkdock_connect_current_project`，明确传入两个路径。例如：

```json
{
  "originalProject": "D:\\repos\\demo",
  "executionDirectory": "D:\\repos\\demo\\worktrees\\feature-a",
  "environment": "windowsLocal"
}
```

`originalProject` 是用户原始选择的工程；`executionDirectory` 是这一轮实际编辑、执行或构建的目录。
两者可以不同；ArkDock 只从实际执行目录发现和选择待构建工程，不把修改中的 worktree 偷换成原始仓库。
也允许原始工程是 Windows 路径、实际目录是 WSL 路径；`environment` 描述实际执行目录。
WSL/远程身份会保留，目前不支持其原生预览，不能通过改写路径伪装成本地工程。

可选 `hostSession` 只填写宿主真实提供的会话标识；未知时省略，不编造。它仅供显示和区分声明，
不能提升证据等级，也不是调用其他工具的前置条件。

| 工具 | 行为 |
| --- | --- |
| `arkdock_connect_current_project` | 登记或替换本连接的完整工程声明；不依赖 roots 能力 |
| `arkdock_context_update` | 切换工程或 worktree 时完整替换声明；省略 `hostSession` 会清除旧值 |
| `arkdock_context_renew` | 手动续期；通常由适配器每 5 秒自动完成 |
| `arkdock_context_revoke` | 撤销本连接登记、停止自动重连；再次 connect 才恢复 |
| `arkdock_context_status` | 只读查询传输、认证回执、声明来源及已接受上下文 |

工具成功需先校验服务端共享密钥证明，再收到 ArkDock 的匹配回执。`connected` 只表示管道已连通；
`serverVerified` 表示对端已证明持有配对密钥，`authenticated` 和 `registered` 仅在随后后端接受登记后成立。
`pending` 表示仍在等证明或回执，`acceptedContext` 是最近已接受的快照。
后端拒绝、断连或超时不能当作登记成功。`requiresConfirmation: true` 和
`visibleConversationVerified: false` 表明还需要 ArkDock 中的明确选择。

## 4. 在 ArkDock 中确认并验收

1. 把 ArkDock 贴附到对应宿主窗口，展开工程条。
2. 查看上下文登记卡中的会话、**原始工程**和**实际目录**，确认是当前任务后点击 **连接此工程**。
3. 实际目录存在多个可构建模块时，从普通候选面板继续选择模块。
4. 构建仍经过既有工程信任门禁；未信任工程需要明确选择“信任并构建”。登记不授予执行权限。
5. 让 Agent 调用 update 切换实际目录，确认旧上下文绑定被清除，重新检查并选择新工程。
6. 调用 revoke、结束 MCP 进程或断开管道，确认绑定被清除。即使工程处于未信任或停止状态，也不能继续保留失效的上下文绑定。

未显式上报实际目录时，卡片会单独显示“宿主建议目录”或“启动目录提示”，不将其称为当前对话的实际目录。
核对后可点击“确认使用此建议”；从普通候选列表选择同一上下文工程也会绑定对应登记，断线时同样解除。
多个登记指向同一工程时，应从登记卡确认具体会话；多个 roots 可从普通候选中选择具体工程。
通过“手动选择”或独立验证目录后选择模块属于独立手动关联，不依赖 MCP 登记继续存活。
仅查看或验证其他目录不会解除正在使用的上下文绑定，失败的目录检查也不会改变现有预览。

相同声明的重复更新和兼容 roots 刷新不会无故使明确选择失效。实质声明变化、断连重连或撤销会改变登记身份，
旧选择不能直接套用到新登记。多个会话/宿主复用连接不会因此获得可见会话证明；应用级共享 MCP 连接
只有一份当前声明，后一次工具调用会替换它，应由用户在 ArkDock 中重新确认。

## roots 的兼容行为

宿主声明 roots 能力时，服务器在收到 `notifications/initialized` 后才请求 `roots/list`。
roots 仅是宿主提供的文件系统边界提示，不是“当前对话”证据；无 roots 的客户端仍可使用全部显式工具。
启动目录也是弱提示，不用于猜测原始工程或实际执行目录。

收到 `notifications/roots/list_changed` 后会先清空旧 roots，再读取新列表；失败、超时、非法 URI 或超限
不会保留旧值。断线重连和定期续期也会刷新 roots；晚到的旧响应不能复活已失效快照。
MCP roots 接受 `file://` URI；POSIX/WSL 路径保持身份，不能转换成本地盘符。
其他远程 URI 可经显式工具字段提交，不能冒充协议 roots。

## 实现边界与验证范围

- 本机管道默认 `\\.\pipe\arkdock-context`，JSON 行；凭证只从文件读取并经本机管道发送。
- OS 提供客户端 PID 与进程启动时间；连接拥有自己的登记，不能撤销另一连接的登记。
- 发送凭证或工程路径之前先发送随机 32 字节挑战；对端必须返回匹配挑战的 HMAC-SHA256 证明。
  客户端使用配对密钥及独立域标识计算期望值并作常数时间比较。证明无效、缺失、超限或 3 秒内未返回就断开，
  因此仅抢占固定管道名而不知道配对密钥的进程不能收取凭证或工程路径。该证明不提升工程声明的证据等级，
  也不隔离已能读取同用户凭证文件的恶意进程。
- 帧上限 16 KiB，roots 最多 8 个，路径最多 4096 UTF-8 字节；stdio 单行上限 64 KiB。
- 每帧携带递增序号并收到匹配 ACK；重放被拒绝。心跳间隔 5 秒，登记 TTL 为 15 秒。
- EOF、断连、撤销、凭证轮换与超时都会清除登记；适配器可重连，但新登记需要重新确认。
- MCP 支持明确的版本集合：2024-11-05、2025-03-26、2025-06-18、2025-11-25；未知版本协商到最新支持版本，不盲目回显。
- 不读对话内容，不提供构建、执行命令、发送消息或 Inspector 图像工具。

便携测试用独立子进程、模拟宿主与临时管道验证握手、服务端证明及伪造端点不泄漏凭证/路径、工具、回执、配置可执行性、目录区分、失效和字节边界。
这些测试不能代替 Claude Code、Codex CLI、桌面宿主及真实贴附/构建验收；没有实际执行的宿主或 GUI 验收必须标为 `NOT_RUN`。

协议依据：[MCP 生命周期与版本协商](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)、
[roots 语义与变更通知](https://modelcontextprotocol.io/specification/2025-11-25/client/roots)、
[stdio 传输](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)。
