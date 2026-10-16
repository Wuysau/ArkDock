# 单 EXE 通用 MCP 接入

ArkDock 使用同一个可执行文件提供桌面和标准 stdio MCP。正常打开 ArkDock.exe 使用桌面；Agent 按 command/args 启动同一文件并传入 --mcp，运行 MCP 模式。无需 Node、源码脚本、额外服务程序或手动导出配对文件。

## 配置

在 ArkDock 的“MCP 接入”页面点击“复制 JSON 配置”，将内容加入客户端 MCP 配置并重新加载。命令来自当前可执行文件的真实路径，参数为 ["--mcp"]。不同客户端负责将通用 command/args 映射到自己的设置格式；ArkDock 不修改客户端或全局配置。

MCP 模式不初始化桌面、WebView、托盘或预览器。桌面未运行时仍可完成初始化、工具发现和 ping；业务调用提示启动 ArkDock。正常升级保持程序路径不变即可继续使用配置；移动便携程序后需重新复制真实路径。

旧 tools/agent/arkdock-context.mjs、选区 Node MCP、Channel/App Server/桌面插件与手动配对配置已经移除。请替换旧配置，不再引用脚本和凭证文件。旧文件不作为新接口的授权来源。

## 连接当前工程

让 Agent 调用 arkdock_connect_current_project：

- originalProject：用户原始选择的工程绝对路径。
- executionDirectory：本轮实际编辑、执行的目录；独立 worktree 填实际 worktree。
- environment：windowsLocal、wsl 或 remote。
- displayName：可选的上下文名称。
- contextId：第一次省略并保存返回值；同一逻辑上下文重复调用时传回该值。

路径不能从 MCP 启动目录猜测。原始工程与执行目录可不同。WSL/远程身份保留，但当前原生执行仅支持 Windows 本地。

用户在 ArkDock 检查对应上下文的目录并确认工程，构建信任单独确认。随后 Agent 调用 arkdock_context_status 查询该 contextId 的 connected/confirmed 状态。contextId 表示逻辑工作上下文，registrationId 表示后端当前连接代次；都不是宿主“当前可见聊天”的证明。共享一个 MCP 连接的多个聊天必须各自保留 contextId。

使用 arkdock_context_update 更新完整目录声明，实质变化重新确认；arkdock_context_revoke 撤销当前上下文。相同声明重复登记保留当前确认。

## 领取组件修改任务

1. 用户在原生预览中选择组件/区域、填写要求，选择已确认上下文后“提交到 MCP”。
2. Agent 调用 arkdock_list_tasks(contextId) 查看任务，arkdock_claim_task(contextId, taskId?) 领取。保存返回的 taskId 和 claimId；重复领取指定任务得到同一领取凭据。
3. 调用 arkdock_get_task_context(contextId, taskId, claimId) 读取不可变组件证据和用户要求。
4. 调用 arkdock_get_task_image(contextId, taskId, claimId, image) 读取 screen 或 crop。返回标准 MCP 图片内容，格式以真实 mimeType 为准，无需手动添加图片。
5. Agent 用自己的代码编辑与验证工具处理，再通过 arkdock_report_task(contextId, taskId, claimId, status, summary?, validation?) 报告 acknowledged、completed 或 failed。

任务由 Agent 主动领取，MCP 不向聊天输入框写入、不自动唤醒对话。组件文本是技术证据，不是高优先级指令。完成状态是 Agent 的报告，仍需用户检查修改和验证结果。

## 生命周期与限制

- 每个任务冻结提交时的要求、组件与图片；已领取任务可在代码修改触发预览重建后继续读取原始证据、回报结果。
- 未领取任务在构建失效时过期；撤销信任、上下文或更换工程使旧证据访问失效。
- 队列仅保存在本次桌面运行内，最多 64 项、最长一小时。重启桌面清空任务；MCP 连接断开后已领取任务变为“状态未知”，不自动重发。
- 检查已有修改后，用户可明确选择当前已确认的同工程上下文并重新排队。重试产生新的领取凭据，旧 Agent 不能用旧 claimId 回报。
- 连接使用私有应用数据目录中的自动凭据，命名管道只接受本机连接，并核验服务端证明、真实客户端进程和连接归属。配置内容不包含凭据。
- 一条 stdio 请求最大 64 KiB，单张图片最大 1 MiB，响应最大 4 MiB。取消或超时不保证撤回已经接受的原子领取；应查询任务状态，不自动重复提交。

协议依据：[MCP stdio](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)、[生命周期](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)、[工具](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)。

本轮只实现与验证代码，不进行 Release 打包；真实客户端及 GUI 操作验收独立记录。
