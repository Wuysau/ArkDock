# Architecture

ArkDock is a local-first Tauri 2 desktop application. Rust owns native windows, project discovery, trust, build scheduling, Previewer processes, Inspector queries, and frame validation. React renders backend snapshots and sends typed requests through the single frontend native entry point, `src/ipc/client.ts`.

```text
Windows window events ──> target lifecycle ──> project evidence and user choice
                                             └─> explicit execution trust
                                                    └─> serialized build session
                                                         └─> real Previewer frames
                                                              └─> typed IPC ──> React
```

## Key boundaries

- `protocol/model.json` is the IPC source of truth. `tools/generate-ipc.mjs` creates the Rust and TypeScript models; generated files are checked in and verified for drift.
- `src/ipc/` is the only product frontend entry to native commands. Components do not invoke Tauri directly.
- `src-tauri/src/windowing/` manages independent top-level windows, docking, layout, effects, and lifecycle.
- `src-tauri/src/discovery/` reads project evidence and returns candidates without running project configuration or build scripts.
- `src-tauri/src/workspace.rs` owns project identity, explicit trust, and persisted preferences.
- `src-tauri/src/scheduler.rs` serializes builds and rejects stale sessions, tasks, and frames.
- `src-tauri/src/preview/` probes supported toolchains and owns native Previewer execution and frame transport.
- `src-tauri/src/selection.rs` and `src-tauri/src/agent/` manage scoped Inspector selections and explicitly available Agent transports.

## State and data flow

Window identity is represented by opaque runtime tokens; raw native handles are not exposed to React. Project identity retains platform/environment, workspace, application, module, and build-configuration context. A previous association or project name cannot establish the current target's active project.

Build, preview, and selection operations carry the current target and session scope. The backend validates the scope and rejects late results. Binary frames are validated against workspace, session, build, generation, sequence, dimensions, and payload before display. A build failure may retain a visibly stale image, but stale pixels cannot receive input as if they belonged to the latest build.

Inspector selection uses backend-observed frame and component metadata. Source locations remain candidates until independently verified against the current trusted project. Agent session labels and manually entered thread IDs do not grant conversation write access; only an implemented, authorized transport with a matching receipt may report a submission.

## Native preview

Preview output must come from a real HarmonyOS Previewer runtime. The frontend frame store supports bounded binary frame transport and rejects stale frame identities. Browser-only UI development reports that the native bridge is unavailable and does not manufacture ArkUI pixels.

Toolchain identity is separate from project trust. Missing or unverified SDK/protocol support blocks native execution with a reason. ArkDock does not redistribute the HarmonyOS SDK or third-party tool binaries. See [Compatibility](compatibility.md) for current product claims and [the frontend contract](native-preview/frontend-contract.md) for the binary frame and IPC interface.

## Docking surfaces and motion

显示模式由同一 WindowController 管理，与工程/构建正交：expanded、collapsing、orb、expanding、hidden。
Orb 是轻量 Win32 独立顶层窗口，不缩小 main WebView，不创建第二套预览/监听；native 菜单与
tray 仅把有限的显示动作排入 actor，不拥有 Tauri IPC 权限。隐藏有真实 tray 注册与显示表面确认门槛。
主动收起与目标最小化的 hidden_by_us 分开；挂起 JointGroup，按当前几何所有权安全释放目标占用。
展开重新核验目标生命周期，并尊重收起期间用户的新布局；运行期 HWND/PID 不持久化。

协议模型版本 2 新增显示状态、motion 偏好、原生 DockConfirmation。水波用 bindingId 去重；
不同目标/解除/减少动画取消装饰。粒子只由实际原生候选激活，单个 premultiplied BGRA layered
辅助窗口经独立线程绘制；最新位置邮箱与有界粒子数控制背压。无候选停止帧调度，WinEventHook 只排队。
get_motion_metrics 通过独立低频受控请求读取累计计数，不随每帧更新 AppSnapshot。
原生绘制的几何拒绝与真正 API 绘制失败分别处理：拒绝暂不绘制，下一有效采样恢复；
真正失败才启用静态 Outline 降级。临时粒子置顶有生产者租约，取消/减少动画/隐藏撤销；
Orb 的置顶仅在入口可见时存在，main 保持普通 Z 序。延迟取消补偿优先于旧 JointGroup 的跟随与解绑恢复。
