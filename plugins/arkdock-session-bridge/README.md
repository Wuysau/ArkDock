# ArkDock Session Bridge

这是待用户显式启用的本地 Codex Desktop 插件。完整预览保留在 ArkDock；插件只提供一个按会话隔离的通信面板。

`plugin.json` 是 portable 插件清单；`mcp.json.example` 是待填写的模板，不会被自动加载。使用项目 Node >= 24.18 运行以下命令，可生成包含本机绝对路径的待审配置：

```powershell
node tools/agent/codex-desktop-plugin.mjs --write-config 'D:/absolute/ArkDock/artifacts/agent-delivery/desktop-config' --pairing-file 'C:/absolute/arkdock-pairing.json'
```

命令只写入新的项目 artifacts 子目录，不安装插件，不改 `.agents`、`.codex` 或全局配置。用户授权安装后，将生成的 marketplace 条目合并到仓库自己的 `.agents/plugins/marketplace.json`，通过宿主插件目录安装/启用。配对 JSON 由 ArkDock 明确创建，令牌仅由本地 MCP 服务读取。

必须具备宿主 `ui/message` 能力、受宿主保护的 `openai/session` 元数据以及唯一匹配的本地 workspace root。缺少任何一项会阻塞连接。服务端不会采信 UI 参数自称的会话或工程身份。

当前真实 Desktop 接入尚未验收。安装与配置、回执语义和兼容边界详见 [Codex transports](../../docs/agent-delivery/codex.md)。
