# 官方 Previewer 架构核查

以下是对固定公开源码快照的技术说明，不是商业 SDK 兼容认证。没有构建或执行外部仓库代码，也没有复制社区实现。

## 固定来源与许可

| 仓库 | 已读取的提交 | 提交时间（UTC） | 许可 |
| --- | --- | --- | --- |
| [ide_previewer](https://github.com/openharmony/ide_previewer/tree/8b86305d7853d96aee92aeccab2c2daafa0d2f5b) | `8b86305d7853d96aee92aeccab2c2daafa0d2f5b` | 2026-09-24 | [Apache-2.0](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/LICENSE) |
| [arkui_ace_engine](https://github.com/openharmony/arkui_ace_engine/tree/21b150e7bb1d5308846f8e5be8ef0482fb433315) | `21b150e7bb1d5308846f8e5be8ef0482fb433315` | 2026-10-07 | [Apache-2.0](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/LICENSE) |
| [arkcompiler_ets_frontend](https://github.com/openharmony/arkcompiler_ets_frontend/tree/3271e689443900dbce3ad766c881f397a38e2c76) | `3271e689443900dbce3ad766c881f397a38e2c76` | 2026-10-08 | [Apache-2.0](https://github.com/openharmony/arkcompiler_ets_frontend/blob/3271e689443900dbce3ad766c881f397a38e2c76/LICENSE) |

这些是分别固定的 master 快照，没有核验同一个 OpenHarmony manifest 下的配套构建。它们不能直接等同于商业 HarmonyOS SDK `26.0.0.105`，也不能证明商业二进制包含同一命令、行为或缺陷。ArkDock 商业适配器仍需按本机工具版本、签名、摘要和真实运行结果准入。

## 原生运行链路

1. `BUILD.gn` 将 rich 目标输出为 `Previewer`，入口是 `RichPreviewer.cpp`；lite 输出为 `Simulator`，入口是 `ThinPreviewer.cpp`。两者的设备能力和引擎不同，lite 实验不能证明 ArkTS rich 支持。[构建目标](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/BUILD.gn#L60)
2. rich `main` 解析参数、连接命令管道、启动命令线程、设置虚拟屏幕、应用配置，调用 `JsAppImpl::InitJsApp`。没有要求启动 DevEco GUI；有效工程产物、SDK 原生库及桌面图形环境仍是前置。[入口](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/RichPreviewer.cpp#L99)
3. `JsAppImpl::Start/RunNormalAbility` 初始化虚拟屏幕、帧 WebSocket、GLFW 和 `AceAbility`，以 `CreateSurfaceNode(..., VirtualScreenImpl::Callback)` 接收原生渲染回调，进入原生事件循环。[运行时初始化](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/jsapp/rich/JsAppImpl.cpp#L85)
4. `AceAbility::InitEnv` 加载资源与 Stage 页面配置，通过 `AceContainer::RunPage` 运行页面并驱动原生引擎。宿主 WebView 中的 canvas/image 仅显示这些真实像素，不能自行生成替代 ArkUI 页面。[资源和页面初始化](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/adapter/preview/entrance/ace_ability.cpp#L293)

```mermaid
flowchart LR
  A[可信工程与配套编译工具] --> B[编译产物和资源]
  B --> P[自有 Previewer 原生进程]
  H[ArkDock Rust 会话宿主] -->|命令：宿主为管道服务端| P
  P -->|帧：Previewer 为回环 WS 服务端| H
  H -->|经校验的二进制像素与会话元数据| U[独立 ArkDock 顶层窗口]
  U -->|局部指针、键盘、文本| H
```

Previewer 中的 mock 层适配输入和部分系统服务；这不把浏览器控件变成 ArkUI，也不保证真机系统 API、生命周期、NAPI 或设备行为完全一致。

## 编译、监听和更新的边界

`arkcompiler_ets_frontend` 的 `es2abc` 入口为 `main -> aot::Run -> Options::Parse -> Compiler::CompileFiles -> ResolveAndGenerate`。其选项包含增量缓存 `--cache-file`、合并 ABC、符号表与 `--hot-reload/--cold-reload` 编译模式。它是编译工具；这些选项没有定义把补丁装入 Previewer 的通道，也没有构成工程的 ArkUI 语法转换、资源处理和 Hvigor 监听协议。[入口](https://github.com/openharmony/arkcompiler_ets_frontend/blob/3271e689443900dbce3ad766c881f397a38e2c76/es2panda/aot/main.cpp#L350)、[选项](https://github.com/openharmony/arkcompiler_ets_frontend/blob/3271e689443900dbce3ad766c881f397a38e2c76/es2panda/aot/options.cpp#L481)

已检查的 Previewer 命令注册表没有名为 `Reload`、`HotReload` 或 `watch` 的通用命令。实际存在的是：

| 入口 | 源码实际语义 | 不能据此推定 |
| --- | --- | --- |
| `ReloadRuntimePage` | `ReplacePage` 页面路由替换 | 重新载入变更的 merged ABC、状态保留或编译成功 |
| `MemoryRefresh` | 把 JSON 提交给异步 `OperateComponent` | 任意源码文件热更新或应用补丁成功 |
| `LoadDocument` | 设置组件预览参数，异步 `ReplaceJSContent` | 普通应用的通用 reload 及更新完成屏障 |
| 引擎内部 `DeclarativeFrontend::HotReload` | 调用路由管理器 `FlushFrontend` | 宿主命令管道可调用这一内部函数 |

命令与确认语义见 [协议核查](protocol.md)。`MemoryRefresh` 的组件代码路径读取 `jsCode/viewID`，Base64 解码 ABC 后执行 `InitAceModule`，替换指定节点；不能把整份应用 ABC 当成这类组件载荷。[组件操作](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/adapter/preview/inspector/js_inspector_manager.cpp#L209)、[ABC 执行](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/frameworks/bridge/declarative_frontend/engine/jsi/jsi_declarative_engine.cpp#L2809)、[内部 HotReload](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/frameworks/bridge/declarative_frontend/declarative_frontend.cpp#L1313)

可以独立实现宿主的进程管理、命令桥、帧中继、会话代次和输入映射。避免每次保存完整打包 HAP 的工程编译路径，要另外读取并实测配套 SDK 的 preview 构建入口、产物目录和缓存行为。状态保留热更新须有单独协议与应用状态证据；不能用命令名或普通新帧代替。

## 社区参考范围

只读参考 [previewer-frontend 的架构文档](https://github.com/ohosvscode/previewer-frontend/blob/0500394a67c6706e8a6d54859a0e19ca821ebc05/docs/architecture.md)，固定提交 `0500394a67c6706e8a6d54859a0e19ca821ebc05`（2026-06-24）。其 Launcher / CommandBridge / FrameRelay / Session 分层可用于理解职责。此快照根目录没有 `LICENSE`，GitHub 仓库许可字段为空；局部目录的许可不能自动覆盖全库。因此没有复制其 Rust、JS、协议文档或探针，也没有以其 lite/Linux 运行声明代替官方源码或 Windows 验收。

源码结构和协议行为应与具体安装工具链分开核验。阅读开源实现不能证明商业 Previewer 使用相同协议，也不能扩大 ArkDock 在 [Compatibility](../compatibility.md) 中声明的支持范围。
