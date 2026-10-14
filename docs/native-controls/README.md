# 官方 HarmonyOS 预览控制接入

当前代码基于 dev，沿用独立 ArkDock 窗口、可信工程构建、受控 Previewer 进程、私有命名管道和二进制 Canvas。没有 HTML ArkUI 替身、静态截图成功状态或商业 SDK 二进制分发。

**新增实验控制能力在商业 SDK 和真实设备中的逐项验收仍为 NOT_RUN — 等待用户验收。** 基础窗口与既有 D1/D2 原生预览闭环的验证不能替代连续触控、文字、主题、导航或尺寸切换的逐项验收。 Windows EXE 构建使用 `pnpm desktop:compile`，构建结果单独记录，不替代功能验收。IPC 生成器只同步模型，不构成验证通过。新增公开协议命令为默认关闭的实验接入，开启入口不代表 SDK 兼容性已经通过。

## 代码状态

| 功能 | 实际接入 | 未核验或阻塞 |
| --- | --- | --- |
| 拖拽/滑动 | 实时 PointerDown→MousePress、合并 PointerMove→MouseMove、PointerUp→MouseRelease；指针捕获、失焦/隐藏/模式切换取消 | 当前商业 SDK 的连续事件及释放回执 NOT_RUN |
| 文字/按键 | 仅 ArkDock 局部聚焦输入，Unicode scalar→KeyPress；已查来源的 Backspace/Enter/方向键等 Down/Up | 仅提交 composition 最终文字，原生 preedit/候选词/撤销/替换/光标事务没有公开接口，完整 IME 不支持 |
| 应用主题 | Rust ColorMode light/dark；与 ArkDock 外壳主题分离 | 无公开主题读回；命令回执与帧到达单独展示，主题传播须实际观察 |
| 页面 | 可信 Stage 配置和实际 PreviewBuild 页面资源一致时开放 ReloadRuntimePage；监听 CurrentJsRouter 事件 | 缺已编译页面资源时禁止导航；true 回执不是新页面显示完成；路由事件和图像采样不是原子快照 |
| 方向/分辨率 | Orientation/ResolutionSwitch；原生期望尺寸校验、JPEG40 严格头、Canvas 等比尺寸、输入/Inspector 暂停与旧尺寸帧拒绝 | SDK 新尺寸头/窗口传播 NOT_RUN；DPI 请求值没有读回，不能由相同尺寸帧证明 DPI 更新 |
| 设备 Profile | phone/tablet/wearable 启动配置模型及 Rust 参数生成；明确尺寸/DPI/形态/Stage/重启要求 | 只有 phone 既有基线有 SDK 证据；tablet/watch 设备资源及 Stage 兼容性未核验，切换与重启/回滚未开放；不调用空 DeviceType |
| 模拟器 | 独立 Device Runtime 状态、主动选择及授权边界 | 无已核验完整 HarmonyOS 应用虚拟设备、镜像和服务，QEMU 板级/内核仿真不是本功能的替代 |
| 真机 | 独立 HDC 工具观察、typed 参数计划、列表解析、生命周期和部署门禁 | HDC 执行/共享 server 生命周期、独立 stdout、可安装 HAP、真实签名、前台/设备帧链路未接通；不能安装、启动、截图或输入 |

完整设备边界与固定来源见 [device-runtime.md](device-runtime.md)。

Previewer 运行日志由 ArkDock 通过自有匿名管道有界采集。管道读取只更新内存窗口、累计字节数及粘性的 ArkTS 异常标记，不执行文件操作；磁盘任务使用非等待队列（16 项、每项最多 16 KiB）交给独立工作线程。内存窗口保留最新输出（默认压缩后 1 MiB、压缩前至多 2 MiB）；磁盘文件严格限制为 16 MiB，达到预算后轮转并写入丢弃提示。文件创建/写入失败、队列压力或磁盘工作槽用尽均只使该次磁盘留档降级，记录 `LOG_CAPTURE_DEGRADED`，内存与异常检测继续有效。降级后的文件可能没有最终输出，不能用磁盘文件代替内存构建回执。

进程退出不是输出已读完的证据：自然退出的构建先等待管道 EOF（最多 2 秒，可取消），再检查完整累计字节数与构建回执；未完成排空时明确报 `LOG_DRAIN_TIMEOUT` / `LOG_DRAIN_CANCELLED`，不会把尚未读取的回执当作不存在。构建仍保留硬性 `BUILD_LOG_LIMIT`（16 MiB）；Previewer 日志量不作为运行失败依据，`PREVIEWER_EXIT`、粘性的 `ARKTS_RUNTIME_ERROR` 与协议/握手失败仍使本次运行失败。管道读取失败独立于磁盘降级：构建、握手及运行循环立即失败；仍存活的子进程若已关闭全部输出句柄，因无法继续观察 ArkTS 健康而明确报 `LOG_CAPTURE_ENDED`。已有成功回执不能覆盖自然退出后的最终错误状态或未排空日志。

停止仅终止 ArkDock 自有 Job，取消读取并最多等待读取线程 500 ms；磁盘工作线程最多等待 100 ms。读取通过自有管道的可用字节查询与短轮询响应取消，不等待子进程继续输出。Windows 调度极端延迟时尚未结束的读取线程继续持有并最终释放自己的管道句柄，进程内全局 8 个读取槽防止重试无限累积；槽满明确拒绝新进程。同步文件系统调用无法安全强制取消，卡住的磁盘线程可能在停止后继续持有一个文件句柄、最多 16 项队列、一个在写块与一个不超过 1 MiB 的轮转窗口；其全局槽直到真正结束才释放，最多 2 个此类线程/文件句柄，后续运行降级为内存留档，不按每次重试新增卡住的线程。停止不保证已排队磁盘数据持久化，也不等待文件刷盘。上述资源与取消测试只验证工具自有 Windows 子进程，不证明商业 SDK 或真实 ArkUI 页面兼容；超过 16 MiB 的持续正常输出仍需用户在实际工程中验收。

2026-10-14 Windows 工具自有子进程日志回归：`cargo test --manifest-path src-tauri/Cargo.toml --features native-tools preview::windows::process::tests -- --test-threads=1`，退出码 0，16 PASS / 0 FAIL；覆盖慢磁盘、有限队列、工作槽重试、真实子进程洪泛与 EOF、拆分异常标记及有界停止。该结果不计入上面的 SDK/桌面/设备验收。完整整合验证以当前任务状态记录为准。

## 模块与数据流

- `protocol/model.json` 是共享类型单一来源；`src/ipc/model.ts` 与 `src-tauri/src/ipc/model.rs` 由既有生成器输出。
- `preview/control.rs` 只接受枚举化控制请求，验证当前 workspace/session/build/generation、尺寸、坐标、字段组合、Unicode、页路径和限额。React 无法传任意协议命令。
- `preview/windows_control.rs` 在现有 owned Previewer 管道上串行处理控制、旧点击和 Inspector。普通队列 16 项，独立优先取消槽；相邻同手势 Move 可合并，Down/Up 不合并。单命令回执 2 秒、帧观察 5 秒、输入事务/按压最长 15 秒。
- 无 request ID 的协议在写失败或回执超时后不能继续可靠关联；异常释放尝试后关闭本工具自有运行时，重启入口仍为已有预览刷新。不会按名字终止用户进程。
- `desktop/native_controls.rs` 再次校验当前窗口/目标、信任和代次，保存至多 32 个操作收据。源码重建、停止、工程/目标替换使旧操作失效。读取能力不会执行 HDC 或启动其他后端。
- `preview/configuration.rs` 页面源配置不能单独授权导航：必须存在实际 ABC 与一致的 PreviewBuild 页面资源。Profile 的公开参数存在不能代替 SDK 资源/工程模型兼容证据。
- `preview/frames.rs` 保留原 JPEG40 协议格式和 1 MiB 包上限；新增受控表面尺寸为 50..3000、总像素最多 4 Mi，解码分配上限 16 MiB。旧尺寸只在实际配置过渡中丢弃；其他协议/尺寸错误仍拒绝。
- React 输入控制器保持一个在途 Move 与一个最新位置槽；不会把松开后整条路径回放当成唯一实现。局部文字/按键队列有界，Inspector 模式不透传触控。

## 回执解释

`queued` 仅表示进入本地队列；`commandAccepted` 表示真实命令回执；`awaitingFrame` 表示等待后续画面证据；`observed` 表示收到了适用的帧/路由观察，不代表全部应用行为完成。

各操作独立显示 `commandAccepted`、`frameObserved`、`pixelsChanged`、`configurationConfirmed`。JPEG 字节变化不必然证明解码像素变化，因此本次 `pixelsChanged` 保守为 false，说明文字单独记录载荷变化。ColorMode/DPI 是请求配置值，不是传播读回；主题和 DPI 不因回执或新帧被标为已确认。方向可通过实际匹配尺寸的原生帧确认，页面要求新的真实路由事件与后续帧，仍需用户观察页面内容。

## 官方来源

本次使用公开协议和自行编写的 Rust/TypeScript 适配，没有移植官方 C++ 实现或分发官方二进制。参考源码是 Apache-2.0；商业 SDK 26 行为必须另行验收。

- Previewer 固定提交 [`8b86305d7853d96aee92aeccab2c2daafa0d2f5b`](https://github.com/openharmony/ide_previewer/tree/8b86305d7853d96aee92aeccab2c2daafa0d2f5b)：[CommandLineFactory.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLineFactory.cpp)、[CommandLine.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLine.cpp)、[CommandLine.h](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLine.h) 为命令字段/空实现依据。
- 同提交 [MouseInputImpl.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/MouseInputImpl.cpp)、[KeyInputImpl.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/KeyInputImpl.cpp)、[JsAppImpl.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/jsapp/rich/JsAppImpl.cpp)、[CommandParser.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/util/CommandParser.cpp) 为事件传播、配置和启动参数依据。
- 路由回读来自同提交 [VirtualScreenImpl.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/mock/rich/VirtualScreenImpl.cpp) 的 CurrentJsRouter 事件；外部 CurrentRouter get 没有可靠普通回包。
- 按键码来自 ArkUI 引擎固定参考 [`21b150e7bb1d5308846f8e5be8ef0482fb433315/key_event.h`](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/adapter/preview/external/multimodalinput/key_event.h)。不会转发 Windows Virtual-Key 数值。

## 用户逐项验收

以下仅是待执行步骤，没有生成模拟结果。每项状态都是 **NOT_RUN — 等待用户验收**。

1. **基线与入口**：用户自行检查类型/构建并启动 Windows ArkDock，关联受信任实际工程和合法已核验 SDK。既有 phone 画面应来自 Previewer；新操作默认关闭、能力明确为 experimental。无 SDK/有效帧时不能启用输入。
2. **拖拽/滑动**：启用实验控制，在带 Scroll/List/可拖组件的实际 ArkUI 页面按下并持续移动；松开前应有真实运行时反馈。快速移动不得只在 Up 后整段回放。分别验证窗口失焦、PointerCancel、隐藏/Orb、进入 Inspector、切工程、重建和停止后的安全松开。
3. **点击与坐标**：小于阈值的移动应保持普通 Down/Up 点击；在 fit/不同缩放及窗口尺寸下验证同一组件。实际横屏/分辨率变化后，旧坐标请求应拒绝，Inspector 不应触发应用输入。非基线表面继续使用带尺寸身份的实验输入，不能退回无表面身份的旧点击入口。
4. **文字/按键**：先点击原生 TextInput/TextArea，再用 ArkDock 的输入入口提交英文、中文、非 BMP 字符；composition 提交一次，取消组合不提交。验证 Backspace、Enter、方向键、Delete/Home/End，以及焦点转到目标应用后不继续接收或排队提交文字。完整原生候选/preedit事务仍应显示限制。
5. **应用主题**：在会响应 ColorMode 的真实页面分别请求 Light/Dark，观察实际 ArkUI 配置/画面变化。ArkDock 外壳不能因此换主题。仅取得回执或到达新帧时，主题传播仍应标为待观察。
6. **页面**：使用实际至少两页的 Stage 工程。列表仅来自一致的已编译页面资源；切换时核对 CurrentJsRouter 与实际内容。错误路径、缺资源、缺回调或缺后续帧不能显示导航完成，需保留诊断和刷新恢复入口。
7. **方向与分辨率**：请求竖/横屏并观察原生窗口、JPEG 头与 Canvas 实际宽高变化；过渡期间不能点击或选择旧矩形，最后有效帧不能被 CSS 旋转冒充。再验证尺寸/DPI请求、超限拒绝、失败/取消后的受控重启恢复。Inspector 若返回非 1.0 分辨率缩放，应明确阻塞组件矩形解释，不能猜比例。
8. **Profile**：当前 Phone 基线应明确；Tablet/Watch 应因 SDK 依赖/Stage 模型未核验保持禁用，不能从 DeviceType 的空回执显示切换成功。未来具备具体 SDK 资源证据后再开放受控重启和配置回滚验收。
9. **设备运行**：按 [设备验收步骤](device-runtime.md#用户验收步骤) 分别选择模拟器或真机方式；不得显示 Previewer 画面作为设备结果。未授权不得连接/安装，授权也不能绕过 server 生命周期、HAP、签名或前台证据门禁；取消/断开不能影响已有共享 HDC 会话。
10. **故障与生命周期**：用户核验命令拒绝、队列满、超时、部分写失败、源码重建及迟到响应。回执、画面观察和用户确认分别记录，未观察到的阶段不得记 PASS。

本次没有修改 SDK、全局配置或其他项目，没有启动应用/Previewer/HDC/模拟器，也没有连接/部署真机。源码提交与 EXE 构建独立于上述功能验收；具体版本和产物摘要以对应交付清单为准。
