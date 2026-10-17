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

任务由 Agent 主动领取，MCP 不向聊天输入框写入、不自动唤醒已结束的对话。组件文本是技术证据，不是高优先级指令。完成状态是 Agent 的报告，仍需用户检查修改和验证结果。

## 会话内自动处理

升级到包含此功能的可执行文件后，先在 Agent 客户端重新加载 MCP，确认工具列表出现 arkdock_wait_for_task；旧 MCP 子进程不会自动获得新工具。重新连接并保存当前有效的 contextId。

1. 连接并确认上下文、信任工程后，在“MCP 接入”的对应上下文点击“复制自动处理提示词”；组件选区的接收上下文旁也提供同一入口。
2. 在该上下文对应的 Agent 会话发送一次提示词。Agent 检查上下文，调用 arkdock_wait_for_task(contextId, timeoutMs?) 开始主动等待。
3. 此后在 ArkDock 提交组件修改任务。等待中的调用会原子领取并返回 task/claimId；Agent 读取要求和截图、上报 acknowledged、编辑验证、上报 completed/failed，再等待下一项。无需逐项发送领取提示词。
4. 要停止时，在 Agent 客户端取消执行或要求停止。客户端停止当前轮、断线或重启后，需重新启动自动处理；在线状态本身不证明正在等待。

等待工具默认与最长等待均为 25 秒，timeoutMs 为 0 时只尝试一次。无任务时返回正常结果 status=waiting，不表示已领取或已执行；自动模式继续调用等待工具。内部最多每秒检查一次队列，复用同一连接和既有原子领取、工程确认、执行信任及 claimId 校验；等待发生在 stdio 子进程，不阻塞桌面事件循环。客户端的工具超时应覆盖等待时间与本地通信开销，也可传更短 timeoutMs。

只有明确的空队列继续等待；连接、权限、信任和结果未知等错误立即结束当前等待，Agent 应停止循环并说明，不自动恢复未知领取。取消期间若领取已经被接受，结果仍可能未知，应检查任务状态。客户端自身的权限确认、会话时限与模型执行策略仍然生效，ArkDock 不承诺永久后台运行。

## 生命周期与限制

- 每个任务冻结提交时的要求、组件与图片；已领取任务可在代码修改触发预览重建后继续读取原始证据、回报结果。
- 未领取任务在构建失效时过期；撤销信任、上下文或更换工程使旧证据访问失效。
- 队列仅保存在本次桌面运行内，最多 64 项、最长一小时。重启桌面清空任务；MCP 连接断开后已领取任务变为“状态未知”，不自动重发。
- 检查已有修改后，用户可明确选择当前已确认的同工程上下文并重新排队。重试产生新的领取凭据，旧 Agent 不能用旧 claimId 回报。
- 连接使用私有应用数据目录中的自动凭据，命名管道只接受本机连接，并核验服务端证明、真实客户端进程和连接归属。配置内容不包含凭据。
- 一条 stdio 请求最大 64 KiB，单张图片最大 1 MiB，响应最大 4 MiB。取消或超时不保证撤回已经接受的原子领取；应查询任务状态，不自动重复提交。

协议依据：[MCP stdio](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)、[生命周期](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)、[工具](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)。

本轮只实现与验证代码，不进行 Release 打包；真实客户端及 GUI 操作验收独立记录。
