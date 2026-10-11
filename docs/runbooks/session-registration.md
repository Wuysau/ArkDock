# 可选会话线索登记

`arkdock-session` 是显式调用的 Windows 本地 helper。它通过结构化参数写入
`%LOCALAPPDATA%\ArkDock\sessions` 的短期 JSON 文件，`resolve` 只读这些文件。
不修改 Shell profile、不注入客户端、不安装插件、不启动构建，也不要求客户端品牌。
Cargo 自动发现 `src-tauri/src/bin/arkdock-session.rs`；需要可用 Rust 工具链自行构建：

```text
cargo build --manifest-path src-tauri/Cargo.toml --features native-tools --bin arkdock-session
```

此 helper 提供弱候选登记，不是经过认证的命名管道协议。发送方 PID 是用户传入且
文件可被同用户伪造，PID/HWND 也不能证明当前活动终端标签。所有记录均强制成为
`verifiable=false`、`activeScope=false` 的候选；需要在 ArkDock 中显式选择并单独确认信任。
命名管道认证与活动范围功能目前 BLOCKED，不宣称已满足完整 B2 验收。

## register、heartbeat、revoke

从当前 ArkDock 目标快照取得不透明目标 token、目标 PID 与十进制进程启动 token。
`--pid` 为维持该登记的长寿命 Shell/客户端进程 PID；不要使用瞬间结束的 helper PID。
Windows 的启动 token 使用 FILETIME，不是 Unix 时间。参数均作为数据处理，不拼接命令。

以下是命令格式；尖括号占位符需替换成当前快照和当前终端的真实值：

```text
arkdock-session.exe register --workspace "C:\work\MyHarmonyApp" --target-id <target-token> --target-pid <target-pid> --target-started-at <decimal-start-token> --pid <shell-pid> --ttl-ms 30000
arkdock-session.exe heartbeat --session-id <returned-session-id> --pid <shell-pid> --ttl-ms 30000
arkdock-session.exe revoke --session-id <returned-session-id>
```

`register` 返回 `status=WEAK`、`sessionId`、`expiresAt`。默认 TTL 30 秒，合法范围
1–120000 毫秒；过期后不会被发现，用户可主动 heartbeat 延长。heartbeat 重查原发送方
与目标进程启动 token，不能把 PID 复用后的新进程续接到旧会话。revoke 仅删除该显式
会话对应的 ArkDock 文件；进程退出、目标变化或时间过期也会使记录失效，不恢复旧窗口。
已有会话 ID 不能被重新指派给不同发送方、目标、环境或路径；换工程先 revoke，再新登记。

可明确指定 `--environment windowsLocal|wsl|remote`。Windows 本地要求盘符或 UNC
绝对路径；WSL 要求其原始 POSIX/WSL UNC/`wsl://` 路径；远程要求
`ssh://` 或 `vscode-remote://` URI。不能把 WSL UNC 冒充 Windows 本地。
环境身份被保留；登记不增加 WSL/远程执行支持，也不会访问网络进行验证。

## 受限读写与生命周期

JSON schema 拒绝未知字段，包括自称 `activeScope`、`authenticated` 的字段。
记录至多 16 KiB，目录枚举至多 128 项；超界失败关闭，拒绝直接的目录/文件符号链接。
会话 ID 仅允许 16–128 个 ASCII 字母、数字、横线或下划线，文件名由其 SHA-256 产生。
读取时核对文件名与记录内会话 ID 的哈希一致。
刷新先写 helper 专属临时文件，再替换显式会话文件；不清理其他应用或批量删除过期文件。
目录的同用户可写性质不提供发送方认证；以上校验只减少错误登记和资源滥用。

Rust Windows 后端以 `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` 与
`GetProcessTimes` 检查当前 sender/target PID 的创建时间和退出时间；访问拒绝直接拒收。
记录必须匹配当前目标 token、PID 和启动 token，且观察时间不得在未来。
非 Windows 平台返回 `BLOCKED` 并退出 3；`--help` 可在任意平台显示格式。

升级为强登记需要独立完成并验证本地命名管道服务：同用户安全描述符、
`GetNamedPipeClientProcessId` 获取真实客户端 PID、客户端/目标生命周期与授权范围、
有界消息、TTL、撤销、活动标签证明、并发及假冒攻击原生测试。缺少其中证据时保持弱候选。
`arkdock-session` 是通过 `native-tools` 启用的可选源码开发工具，Windows 安装包不包含它。基础贴边和目录选择不依赖该 helper。使用修改后的构建前，应先在 Windows 上运行公开质量检查。
