# 架构

单一 Tauri 2 桌面项目，Rust 拥有窗口、文件、信任、发现、监听和原生能力；
React 只发类型化请求并显示后端快照。协议定义在 `protocol/model.json`，
生成 TypeScript 与 Rust serde 类型，检查生成结果与来源一致。

```
Windows 拖动/事件 → windowing → 目标生命周期
                                ↓
会话登记/显式选择/弱线索 → discovery → 候选与可解释决策
                                ↓
                        workspace/独立信任确认
                                ↓
实际工程监听 → scheduler(代次/单任务) → preview(探测/能力/启动)
                                ↓
                   Rust 快照/诊断/真实帧 → IPC → React
```

窗口标识为运行期不透明 token，原生句柄不暴露前端。进程启动时间与 PID
一起核验；重启不恢复 HWND。ProjectRef 包含环境、工作区根、应用根、模块、
变体、SDK 身份。规范化应用路径 + 环境 + 模块形成身份，不按仓库名去重。
运行期 SessionRef 保存当前目标/进程生命周期、ProjectRef、绑定证据与 SDK 身份，
只在内存/后端快照存在；不写入历史设置。RequestScope 防止迟到操作跨会话生效。

状态：未绑定 → 发现中 → 候选待选/未信任 → SDK 缺失/协议阻塞 →
构建中 → 运行中/旧画面/崩溃。停止、解除与切换取消旧会话资源。
快照 revision 单调增长，UI 忽略迟到快照；帧校验 session/build/generation。

登记采用受控本地协议。发送方生命周期、目标范围、过期和活动范围均须核验。
无法证明活动工程范围时仅给候选。Windows UIA 已只读采集选中标签元数据，但
activeScope=false；启动参数/认证登记与宿主工作区范围证明未接通，默认没有自动关联来源。
历史是排序与显式确认辅助，不自动切换；多弱来源不能升级成强证据。

信任与发现分开。信任记录绑定工程身份/构建配置指纹，未知 SDK 配置禁止执行。
SDK 诊断只读。UI 不拥有任意命令、磁盘扫描或句柄。
后端自定义命令逐一检查 main 窗口与当前 token，不依赖 capabilities 自动保护。

HarmonyOS 26.0.0.105 Windows 适配器已有商业二进制实测依据：异步执行 SDK 自带
Hvigor PreviewBuild，使用模块 .preview 中真实 modules.abc 启动 Previewer，私有命令管道传点击，
核验自有回环 WebSocket 的 40 字节头/JPEG。默认转换为 PNG，经现有事件入口和独立 FrameStore 显示；
也提供显式协商、会话绑定的 ArrayBuffer JPEG 读取，正式 UI 保留兼容入口。
构建图/首帧完成才提交成功；源码变更串行预览增量构建并重启，代次失效后拒绝旧完成/帧/输入。
失败保留旧画面并禁用输入；停止/换工程通过 Windows Job 回收自有进程树。
仅默认 Stage 目标、pages/Index、360×640 和点击；其他版本/能力明确拒绝。
正式路径没有 mock 后端，配置指纹不是验收成功开关；native gate 每次执行真实断言。
工程信任与工具链状态分离：SDK 缺失或协议阻塞关闭执行资源，但不撤销有效工程配置授权。
桌面安装包仅分发主应用与许可资源。两个源码 CLI 须显式启用 native-tools；基础门禁保留其测试，桌面不调用它们。
具体范围见 [SDK 证据](evidence/sdk-26-preview.md) 和 [本次集成交付](evidence/integration-delivery-2026-10-10.md)。
