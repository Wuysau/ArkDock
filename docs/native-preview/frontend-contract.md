# 前端接入契约

前端已接入二进制 JPEG Canvas、互斥交互/检查模式和选择上下文。
共享类型唯一来源为 `protocol/model.json`，执行 `node tools/generate-ipc.mjs`
生成 `src/ipc/model.ts` 和 Rust 类型；不要手写另一份字段或命令定义。

## 已存在的桌面入口

前端原生能力入口仍为 `src/ipc/client.ts`。`snapshot` 返回 `AppSnapshot`；
除下述二进制读取外，其他命令通过 `request` 传入当前 `RequestScope`，返回更新后的快照。
`observe` 订阅 `arkdock:state`、兼容 `arkdock:frame` 及 `arkdock:visibility`，卸载时释放三项订阅。
没有 Tauri 桥接时明确显示桌面能力缺失，不生成预览替身。

| 操作 | 现有命令及参数 | 后端要求 |
| --- | --- | --- |
| 关联候选 | `select_candidate`：`candidateId`、`scope` | 当前目标生命周期和候选有效；关联与执行信任独立 |
| 信任工程 | `set_trust`：`projectId`、`trusted`、`scope` | 完整工程身份和构建配置指纹；撤销立即结束会话 |
| 选择 SDK | `configure_sdk`：`scope` | 指定合法安装的完整 Command Line Tools 根目录；摘要匹配才具备该配置能力 |
| 刷新 | `refresh_preview`：`scope` | 当前可信会话；单工程串行构建、变更合并、旧任务拒绝 |
| 启用二进制帧 | `enable_binary_preview_frames`：`protocolVersion`、`scope` | 当前可信会话、协议版本 1；只为当前 WebView 显式选择原始帧模式 |
| 恢复兼容帧 | `disable_binary_preview_frames`：`protocolVersion`、`scope` | 释放未读 JPEG 并恢复 `arkdock:frame`；卸载原始帧消费者时调用 |
| 读取二进制帧 | `read_preview_frame`：`protocolVersion`、`scope` | 当前会话、可信/运行状态、精确构建身份；返回原始字节响应 |
| 点击 | `click_preview`：`buildId`、`generation`、`x`、`y`、`scope` | 当前原生画面对应的构建和代次；整数像素坐标在画面范围内 |
| 停止 | `stop_preview`：`scope` | 清除当前会话、帧和输入，释放自有进程及通信资源 |

`scope` 为 `{ targetId, sessionId }`。不要缓存旧 scope 后操作新的目标或会话；
后端会拒绝迟到请求。前端不接收 HWND，不启动 SDK，不访问 Shell 或进程插件。

## 二进制帧与兼容入口

Rust `PreviewBackend::binary_frame()` 取出最新一帧 `BinaryFrame`；JPEG 原始压缩
字节与 `BinaryFrameMetadata` 分离。默认已有 Tauri 事件仍使用 `NativeFrame`
的 PNG/base64 兼容形式；现有前端不需被迫切换。

前端显式调用 `enable_binary_preview_frames` 后，用 `read_preview_frame` 有界
轮询最新一帧。读取返回 `tauri::ipc::Response` 原始字节：零字节表示当前没有
未读帧，有数据时为下述封包。原始模式跳过 PNG/base64 转换；原生图像不写入
React 全局状态。`src/ipc/client.ts` 保留通用 `request` 的快照返回类型，
新增 `readPreviewFrame` 使用生成的 `CommandResults['read_preview_frame']`：

```typescript
const consumerId = crypto.randomUUID();
await request('enable_binary_preview_frames', { protocolVersion: 1, consumerId }, scope);
const packet = await readPreviewFrame(scope, 1, consumerId);
// packet: ArrayBuffer；0 字节表示当前没有未读帧。
// 组件卸载时使用仍有效的 scope 恢复兼容入口：
await request('disable_binary_preview_frames', { protocolVersion: 1, consumerId }, scope);
```

不要从 React 组件绕过该入口直接调用原生能力。共享协议为现有 v1 的增量扩展，
默认兼容路径保留，没有要求现有前端强制迁移。

`enable_binary_preview_frames` 返回更新后的快照，`read_preview_frame` 返回
`ArrayBuffer`/字节数组；不要把后者按 JSON 快照解析。每次请求携带
`protocolVersion: 1` 和最新 scope，串行轮询，卸载或停止时结束轮询。
模式绑定当前可信会话和 consumerId；同一会话旧消费者不能关闭或读取新消费者的流。
每次重新协商生成新的 consumerId；即使控制器和 session 没有变化，也不复用旧 lease ID。
停止、解除、换工程、撤销信任和 main WebView 重新加载
会释放缓存并恢复兼容模式。新会话需要重新协商；旧 scope 不能关闭新会话的传输。
二进制命令失败返回生成类型 `PreviewProtocolError`：`code`、`message`、
`expectedProtocolVersion`、`receivedProtocolVersion`。收到
`PROTOCOL_VERSION_UNSUPPORTED` 时停止读取并显示版本不兼容，不自动猜测格式。

封包布局：4 字节小端元数据长度、UTF-8 JSON 元数据、原始 JPEG 载荷。
`BinaryFrameMetadata` 字段及其意义如下，实际类型从生成文件导入。

| 字段 | 意义 |
| --- | --- |
| `protocolVersion` | ArkDock 二进制封包版本，当前为 1；不等于 SDK 的输入协议版本 |
| `workspaceId`、`sessionId` | 当前工程和执行会话身份 |
| `buildId`、`generation` | 产生该帧的构建及源码代次 |
| `sequence` | 当前构建流内严格递增的帧序号，从正整数开始 |
| `width`、`height`、`rotation` | 当前核验配置为 360、640、0 |
| `capturedAt` | 后端接收并形成该帧的时间戳，Unix 毫秒；不声称为引擎内部渲染时刻 |
| `format`、`payloadLength` | `jpeg` 及原始 JPEG 字节长度，必须与载荷相等 |

JSON 元数据最多 4096 字节。SDK 的完整 JPEG40 消息最多 1 MiB，去掉 40 字节
引擎头后原始载荷最多 1 MiB − 40 字节。二进制封包加上元数据后可略大于
1 MiB；接收端应分别检查两个上限，避免误把封包上限等同于 SDK 消息上限。
后端核验完整 JPEG、尺寸和分配上限后发布；前端仍应在分配或解码前核验封包。

最新帧邮箱只保留一帧。前端应替换显示内容，释放上一张 Blob URL/图像资源；
不积累每帧队列。后端计量 `received`、`dropped`、`rejected`、
`bytes_received` 和 `buffered_bytes`，均为 Rust 内部诊断数据，没有新增 IPC。

## 状态和帧的版本边界

Windows WebView2 最小化时，`document.hidden` 可能仍为 false。新增生成类型
`PreviewVisibility` 的 `arkdock:visibility` 为权威窗口门禁：后台只读取当前进程自有
main HWND 的 IsWindowVisible 与 IsIconic，每 100 ms 核验，变化时通知，并每秒刷新一次
以处理重新订阅；不在后台同步调用 WebView 线程 getter。前端结合原生 visible 与
document.hidden 暂停消费者和 presenter；恢复沿用同一 Canvas/bitmap，再消费最新有效帧。
悬浮球集成时应保持此原生 hide/show 门禁，不用 React 隐藏 class 代替真实窗口状态。

状态通知与帧通知可能在不同时间到达，不能仅凭“最近收到”选画面。二进制帧必须
匹配当前工程、会话、构建和代次；同一流内只接受更大 `sequence`。
PNG 兼容帧缺少 `workspaceId`，仍须通过当前快照的会话、构建和代次核验。
停止、切换工程或撤销信任后丢弃迟到帧。不得给上一构建画面套用新 `buildId`。

| 状态 | 应显示的行为 |
| --- | --- |
| `untrusted` | 可查看只读工程信息；明确授权后才构建 |
| `sdkMissing`、`protocolBlocked`、`unsupported` | 展示后端原因，保留明确阻塞；禁用预览输入 |
| `building` | 显示构建进度；保留旧图时标明其显示代次，并禁用输入 |
| `running` | 当前构建已收到真实原生帧；能力允许时接受当前画面输入 |
| `stale` | 构建失败后保留旧图并说明失败；当前源码未呈现，禁用输入 |
| `crashed`、`stopped` | 展示运行失败或停止信息；释放显示资源并禁用输入 |

点击必须使用实际显示帧的 `buildId` 和 `generation`，并与快照的当前构建
一致。仅 `backend.canClick` 不足以使失败旧画面可点击。
`backend.canSwipe`、`canText`、`canRotate` 目前为 false；不显示为可用能力。
源码刷新使用预览专用构建后重启运行时，组件状态会重置，不声称状态保留热更新。

## 点击坐标

坐标是原生表面的整数像素，当前 `0 ≤ x < 360`、`0 ≤ y < 640`。
前端根据图像实际显示内容矩形进行映射；使用 `object-fit: contain` 时先扣除
留白和缩放，留白点击直接忽略。不要把页面 CSS 坐标、屏幕坐标或 devicePixelRatio
重复缩放后的值直接发给 Previewer。后端再次校验边界和构建身份。

## 接入验证

检查与选择命令使用 `src/ipc/client.ts` 的 `transfer` 返回生成的 `CommandResults`，
不按 AppSnapshot 解析。Inspector 查询异步执行，最多一项在途；前端至少间隔 350 ms，
画面内容变化后合并查询，静态场景约 750 ms 刷新。组件树保留原生矩形、节点、属性
和源码提示，缺失字段为空；JPEG 内容锚点变化后不能继续用旧边界命中。
协议无原子帧标记，`inspectionExactFrameMatch=false`，源码提示目前仅 Candidate。

选择创建只传完整 metadata、区域、节点 ID、树版本和意见；不传 frontend JPEG/属性。
后端从自己的压缩帧归档核验完整 metadata 并截图，最多 8 帧且 4 MiB，与最新未读邮箱分离。
超过该保留窗口会保守拒绝并提示重新选择。源码重建、换工程、停止、撤销信任立即失效。
`selection_update_instruction` 保持意见与 selectionId 关联；图片经专门二进制入口读取。

手工填写的 Agent 线程 ID 不会赋予写入权。当前适配器不支持向活动 Codex/Claude
会话发送；只有已授权的写入通道和匹配提交回执才可显示 `SUBMITTED`。复制、导出、
受显式 grant 约束的只读 MCP 均保留独立语义。

前端完成后，在实际 ArkDock WebView2 验证真实首帧、点击计数、源码变更、
编译失败旧图、恢复和停止。CLI 的后端原生验收不等于前端点击坐标或布局验收。
浏览器 UI 演示、静态截图和发现 fixture 均不能证明鸿蒙原生预览能力。
集成后的能力范围和验证限制见 [Compatibility](../compatibility.md)。
