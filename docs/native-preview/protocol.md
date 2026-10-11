# Previewer 协议与适配边界

本文总结公开 OpenHarmony Previewer/ArkUI 源码中的协议行为，供实现者理解帧、输入和更新边界。源码研究不等于商业 HarmonyOS SDK 兼容认证；ArkDock 只启用经项目准入逻辑识别的工具链配置。通用产品范围见 [Compatibility](../compatibility.md)，源实现背景见 [官方架构](official-architecture.md)。

## 启动与双通道

rich `CommandParser` 的关键参数：

| 参数 | 源码语义 |
| --- | --- |
| `-j <directory>` | 已编译应用目录，必须存在 |
| `-url <page>` | 非空初始路由 |
| `-or <width> <height>`、`-cr <width> <height>` | 各两个整数；原始和压缩尺寸 |
| `-device <type>`、`-shape rect` | 经设备列表/形状校验 |
| `-pm <model>`、`-av <version>` | 工程模型与 Ace 版本 |
| `-arp <directory>`、`-pages <name>` | 应用资源目录与页面配置 |
| `-s <base>` | 命令管道名称基部 |
| `-lws <port>` | 原生帧服务的回环端口 |
| `-sid <hex>` | 可选 WebSocket 路径校验值 |
| `-refresh full/region`、`-cpm true/false` | 帧头扩展与组件模式，须作为组合验证 |

参数通过原生结构化 argv 传递，不拼接 shell。[注册和校验](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/util/CommandParser.cpp#L58)

宿主先创建 Windows 管道服务端 `\\.\pipe\<base>_commandPipe`。Previewer `LocalSocket::ConnectToServer` 用 `CreateFileW` 连接；命令是 JSON 文本加单个 NUL，没有长度前缀。宿主须处理读写拆分、连续消息和限额，不能把一次 `ReadFile` 当作一条 JSON。[Windows LocalSocket](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/util/windows/LocalSocket.cpp#L24)

请求示例（末尾另加 `0x00`）：

```json
{"version":"1.0.1","type":"action","command":"MousePress","args":{"x":120,"y":220}}
```

`type` 是 `set/get/action`。响应可能是 `{version,command,result}`，也可能是主动 `{MessageType,args}`；没有请求 ID。`version` 的源校验只验证版本字符串格式，不等于协商支持该版本全部语义。初始化通知为 `{"MessageType":"imageWebsocket","args":{"port":"<string>"}}`。[命令接口](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLineInterface.cpp#L29)

Previewer 是帧 WebSocket 服务端，宿主连接 `ws://127.0.0.1:<port>/<sid>`，子协议为 `ws`。`CheckSid` 比较 URI 最后一个 `/` 后的片段；没有配置 sid 时源实现放行。WebSocket 接收回调没有实现控制消息处理，输入仍走命令管道。发送采用 binary/continuation，宿主应按完整 WebSocket 消息解析，不能把每个底层分片当成完整图片。[服务实现](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/util/WebSocketServer.cpp#L53)、[回环地址](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/util/WebSocketServer.h#L49)

ArkDock 额外要求随机名称/sid、管道当前进程 PID、回环端口所有者 PID、所有者 ACL、拒绝远程管道客户端，以及自有 Job 生命周期。sid 是局部路径校验，不是多用户强隔离证明。

## 帧头和负载

rich `VirtualScreenImpl::SendPixmap/WriteBuffer` 写出固定 40 字节帧头，整数为 network endian（大端）。该 rich 快照前两组尺寸重复写回调宽高，不能将它误述成所有版本都会携带不同的 original/compressed 尺寸。[写帧](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/VirtualScreenImpl.cpp#L376)、[大端写入](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/VirtualScreenImpl.h#L64)

| 偏移 | 类型 | rich 写入内容 |
| --- | --- | --- |
| 0 | u32 | magic `0x12345678` |
| 4 / 8 | i32 / i32 | 回调宽 / 高 |
| 12 / 16 | i32 / i32 | 回调宽 / 高重复 |
| 20 | u16 | region 模式的 protocolVersion |
| 22 / 24 | u16 / u16 | region 起点 x / y（此 rich 路径为 0 / 0） |
| 26 / 28 | u16 / u16 | region 宽 / 高 |
| 30–39 | 10 bytes | region 保留区，零 |
| 40… | bytes | 图像负载，长度由 WebSocket 消息界定 |

`full` 模式的 20–39 全为零，不能按 region 的 version 字段判断格式。源枚举为 `LOADNORMAL=2/LOADDOC=3/LOADDOCRGBA=4`；rich 组件定时发送路径设置 4，`-cpm` 决定 `SendRgba`，其他路径 `Send` 把 4 字节像素提取 RGB 后 JPEG 编码。模式、版本值、实际负载须共同验证，不能仅按枚举名称推断任意二进制行为。[常量](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/VirtualScreen.h#L72)、[JPEG/RGBA 分支](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/VirtualScreenImpl.cpp#L297)

解码前校验消息长度、magic、正尺寸、最大像素/字节数、矩形范围和已准入模式。JPEG 校验实际解码尺寸与头部；RGBA 用经过验证的行布局及 `width * height * 4` 长度，未知布局拒绝。最新帧邮箱容量 1，前端消费完成/过期均释放二进制 Blob 或纹理；不保留无限帧历史，也不把图像转成 Base64 控制消息。

## 输入

输入命令由设备分支注册。rich 的键盘能力不能由 lite 的触摸实验证明。[注册表](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLineFactory.cpp#L27)

| 命令（type） | 源结构 | 适配限制 |
| --- | --- | --- |
| `MousePress/Move/Release`（action） | `x/y` 整数；映射成触摸 DOWN/MOVE/UP | 原生表面坐标，校验边界；拖动必须含释放 |
| `PointEvent`（action） | `x/y/button/action/sourceType/sourceTool/axisValues`，可选 `pressedButtons` | 必须验证枚举和轴值；不直接转发 DOM/Win32 数值 |
| `KeyPress`（action） | `isInputMethod=false`，`keyCode/keyAction/pressedCodes`，可选 `keyString` | 源限制 code 2000–2119、action 0–2，需按 OHOS 枚举映射 |
| `KeyPress`（action） | `isInputMethod=true`，整数 `codePoint` | 是字符注入；没有提交字符串或完整 composition 事务 |

[命令校验](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLine.cpp#L1391)、[键事件映射](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/KeyInputImpl.cpp#L36)、[指针映射](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/MouseInputImpl.cpp#L47)

源 `EventDispatcher::DispatchInputMethodEvent` 将 codePoint 转为 `wchar_t` 后 `AddCharacter`。Windows 的非 BMP 字符、组合输入、编辑选择/删除和中文 IME 必须单独实测，不声明完整 IME 支持。仅采集预览表面获焦后的局部输入，不读取全局键盘或剪贴板。[字符分发](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/adapter/preview/entrance/event_dispatcher.cpp#L396)

## 更新确认与旧帧

- `ReloadRuntimePage`（set）参数为 `{"ReloadRuntimePage":"<route>"}`。处理函数调用 `JsAppImpl::ReloadRuntimePage`，后者仅在 ability 存在时 `ReplacePage`；响应仍写 true。不能将该响应当作字节码重载成功。[命令](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLine.cpp#L719)、[运行时](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/jsapp/rich/JsAppImpl.cpp#L781)
- `MemoryRefresh`（set）传递组件 JSON；`OperateComponent` 将任务加入 UI 队列后立即返回 true。稍后的失败通过 FastPreview 回调通知。该响应证明已排队，不证明组件更新或新帧产生。[异步执行](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/adapter/preview/entrance/ace_ability.cpp#L604)
- `LoadDocument`（set）需要 `url/className/previewParam`。引擎 `AceContainer::LoadDocument` 将 `ReplaceJSContent` 放入 JS 队列；Previewer 有基于内部单调时钟和 flushEmpty 的选帧逻辑，但没有将时钟、请求 ID 或构建代次放入 wire 帧头。[引擎队列](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/adapter/preview/entrance/ace_container.cpp#L1108)、[内部选帧](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/VirtualScreenImpl.cpp#L79)

命令确认与图片来自两个独立通道，源码没有给出二者的共同顺序保证。WebSocket 重连还会发送缓存中的上一张图片；因此“收到 true 后的第一帧”“重连后的第一帧”或“等固定延迟再收帧”均不足以证明该帧属于新构建。[缓存重放](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/util/WebSocketServer.cpp#L100)

宿主应串行执行会话更新，按命令种类匹配响应并区分主动上报；每个任务携带自己的 workspace/session/build/generation，切换、停止、失败后拒绝迟到结果。给 wire 帧补上本地代次不改变它可能为旧像素的事实。新进程可通过新 PID/管道/sid 将旧会话隔离；同进程更新需要实际 SDK 的完成屏障或可证明的内容/产物关联，缺少时更新能力保持 BLOCKED。不能以重启画面宣称状态保留热重载。

## 解除门禁所需的真实证据

1. 固定商业工具版本/签名/摘要，真实工程构建产物及摘要；记录使用的是 preview 编译还是完整 HAP。
2. 同一原生进程中更改实际 ArkTS，检查更新画面内的独有标记、输入状态及官方日志；独立测量命令返回和第一张可确认更新帧时刻。
3. 插入编译失败、两次紧邻保存、通道重连和停止；旧帧/旧任务均不得升级为当前成功。
4. 明确区分源码支持、单机实验和产品准入；命令不存在、未知格式或无法证明更新顺序时返回 BLOCKED。
