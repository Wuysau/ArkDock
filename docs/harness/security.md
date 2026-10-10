# 安全边界

拖动仅授权选择目标，不授权进程内存、聊天资料、全盘读取或构建执行。
EnumWindows / DWM / 有权限的进程生命周期读取集中于 Windows 模块；
不注入、不 SetParent、不修改客户端、不采集全局键盘/剪贴板、不自动提权。
高权限或拒绝访问目标安全排除，标注降级，不请求管理员权限。

发现只读受限 JSON5 配置，限制祖先、模块、大小和符号链接范围，不执行 TS
配置。窗口、PID、启动目录不等于活动终端标签或实际 Agent 工作区。
来源、时间、目标/进程/会话、环境与可核验范围都保留；弱证据不能升级。
手动文件选择原始/规范化路径必须保留 WSL/remote 身份。

当前文件登记只是同用户弱提示；不是认证通道。生命周期校验和过期防止陈旧，
仍不能防止同用户伪造或证明活动标签。未来强来源需要受控管道 peer 身份、
活动范围、心跳和可靠宿主证明；没有这些不能自动关联。

执行信任独立于发现，绑定 ProjectRef 全身份与根/声明模块构建配置指纹；
配置变更要求再确认。每次执行前再次核验。配置仅存 app data，SDK 路径不进仓库。
无效/未来版本设置保留原件；写入先落临时文件/备份再替换，不静默迁移覆盖。
本机 AppData 发生 Windows rename 错误 17 时，使用自有 preferences 日志：
不可变 JSON 先 sync/close，再写零字节提交标记；只加载最后已提交且摘要有效的记录。
未提交残片忽略，已提交记录损坏则拒绝加载，保留当前/前一记录与旧设置。

Tauri 仅本地 main 窗口有事件订阅权限。自定义命令检查 main 身份、本地 URL 来源、参数上限、
RequestScope 的当前目标/会话；文件选择器打开前和返回后均核验。
前端不接收 HWND，不拥有 shell/fs/process 插件。路径不是授权凭证。
不开放 HTTP/WS 本地执行服务；Vite 调试服务只监听回环。

native-previewer 仅启用已实测的 HarmonyOS 26.0.0.105 Windows 配置，
SDK manifest 与 Previewer/构建入口 SHA256 必须匹配；其他版本仍 PROTOCOL_UNVERIFIED。
构建只在独立信任确认与配置指纹复核后执行，结构化参数不经过 Shell。
CreateProcessW 暂停创建、先加入 kill-on-close Windows Job 再恢复，限制继承的日志句柄，
超时/停止只终止此 Job 的自有子树，不按名称 kill 用户 Previewer/Node/终端。
不自动打开 DevEco GUI、不签名/部署真机、不重新分发 SDK。

原生管道用随机 128 位名称、所有者 DACL、拒绝远程连接，并核验实际客户端 PID。
图像连接只接受经 TCP 表核验的自有 Previewer 回环端口，使用随机 session 路径。
帧 1 MiB、360×640 JPEG 解码/PNG 重编码；命令 1 MiB、输入队列 16、最新帧邮箱 1。
同用户恶意进程仍不属于强隔离安全模型；协议不是供外部客户端执行命令的服务。
具体版本、实验与正式验收分别见 [SDK 证据](../evidence/sdk-26-preview.md)。

状态/帧要求 session/build/generation/sequence，尺寸和载荷限额，最新帧邮箱背压。
事件邮箱保留最后窗口状态并合并文件变化；停止/切换替换旧监听邮箱。
日志关联 workspace/session/build/backend；导出前移除路径和敏感文本。
当前诊断仅本地 UI，无导出功能；artifacts 脚本对 SDK 路径脱敏。
