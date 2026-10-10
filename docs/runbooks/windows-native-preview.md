# Windows 与原生验收操作手册

当前 Windows 构建、单屏窗口/界面及 HarmonyOS 26.0.0.105 原生闭环已实测。
真实商业工具包、签名、摘要、构建与通信依据见 [SDK 证据](../evidence/sdk-26-preview.md)。
实际结果见 [验证记录](../evidence/verification-2026-10-09.md)。

## 1. 本机复现构建

Windows 11 本地 PowerShell。本次 portable 工具放在 D:\CodingTools，
只设置当前进程 PATH；新 Shell 默认 Node 24.13.0 低于项目 >=24.18.0。

```powershell
$env:PATH = 'D:/CodingTools/node-v24.21.0-win-x64;D:/CodingTools/ArkDock-pnpm/node_modules/.bin;' + $env:PATH
$env:ARKDOCK_SDK = 'D:/CodingTools/HarmonyOS/command-line-tools'
node --version
pnpm --version
rustc --version
cargo --version
pnpm exec tauri --version
pnpm install --frozen-lockfile
cargo generate-lockfile --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --all
pnpm verify
cargo check --locked --manifest-path src-tauri/Cargo.toml --features desktop --all-targets
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --features desktop --all-targets -- -D warnings
pnpm run doctor
pnpm dev
```

项目内 @tauri-apps/cli 2.12.1，desktop 包装使用其官方 JS 入口，不需要全局 cargo tauri。
原生测试驱动要求 `ARKDOCK_TEST_PID`（或 `ARKDOCK_PROBE_PID`）为本次明确创建的
ArkDock PID，`ARKDOCK_TEST_PORT`（或 `ARKDOCK_PROBE_PORT`）指定开发调试端口。
驱动核验该端口监听进程为指定 ArkDock 的 WebView2 后代；不会选取机器上第一个同名实例。
SDK 和调试变量仅设在测试进程，不改全局配置。运行中的 Windows exe 会被系统锁定；
需执行 all-targets 测试或重建时，先关闭本次自有实例，或从工作树私有目录的副本启动。
Rust MSVC、Windows C++/SDK、WebView2 按
[Tauri 官方前置](https://v2.tauri.app/start/prerequisites/)。
Cargo.lock 已交付；verify 对 Rust 操作使用 --locked。没有测试 rust-version 的 MSRV 兼容。
重新生成锁需 crates.io 网络或完整缓存；本次最终 generate-lockfile 在线成功，
未改 registry 或关闭校验。早期 schannel 失败记录不作为当前阻塞。

## 2. 真实 Windows 与界面驱动

为了自动化实际开发 WebView2，单独的本地开发进程开启 debug port：
这是测试工具连接，不是原生预览协议或正式产品配置。

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9223'
pnpm dev
# 另一个采用上述 process PATH 的 PowerShell
pnpm verify:windows
pnpm verify:ui
pnpm verify:native
```

windows 驱动创建自己拥有的两个同 PID 的普通 WinForms 窗口，执行真实鼠标拖动、
释放、移动缩放、焦点、最小化恢复、关闭、四边轮廓和明确同意的布局恢复。
目标延迟处理队列时，分别即时/超时后解绑，核验正常与最大化恢复。
驱动只关闭自己创建的 fixture child，最终解除关联；不按进程名结束程序。
报告 artifacts/windows-smoke.json 和截图 windows-smoke.png。

ui 驱动连接实际 ArkDock，检查浅色/深色、刷新持久化、系统媒体变化、320/420
窄窗无横向溢出与导航，截图 ui-light/dark-wide/narrow.png。
界面采用冰蓝玻璃/藏蓝面板；设备轮廓只是空状态装饰，不提供假原生画面。
真实 frame 接收入口仍唯一 IPC；旋转/保留状态热重载控制保持禁用。

测试驱动会移动自有窗口和 fixture。恢复窗口位置时只触碰 ArkDock 自己的窗口。
开发结束清除该进程中的 WebView2 debug 参数并关闭自己启动的 dev 进程；
不要把调试端口配置写成全局环境或正式配置。

另须真桌面验证双屏 100%/150%（含负坐标与不同任务栏）、跨屏 DPI、
目标更高权限、同名/同 PID、多窗口、快速销毁复用 HWND、事件积压。
本机只有一块 192 DPI 显示器；无法执行双屏案例，不能由几何单测替代。
同意并排前目标不动；布局改变、用户开始拖动、生命周期失效后不能恢复旧位置。
已接受的 User32 异步请求无法撤回，超时补偿最多三秒，不承诺失去生命周期后操作外部窗口。

## 3. 发现、信任和监听

Windows UIA 仅读当前绑定窗口选中标签 metadata，有元素/深度/期限限制，
不读取文档/聊天正文。Selection 不证明宿主工程语义，activeScope=false；
标题和文件登记也弱，唯一弱候选仍进入选择。
候选保留完整路径、工作区、环境、模块、来源、有效性与理由，搜索和滚动不截断。

标记 monorepo 显式目录最多下探两层/64 目录/512 条目/16 应用；
全程只读、不执行 hvigor、不跟随越界路径，统一 256 候选/8 MiB 预算。
目录弹窗前后重验 target/session，WSL UNC 不落到 Windows 同名目录。
关联不授权构建，信任绑定配置指纹；修改根/模块构建配置要求重新信任。

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-targets
```

OS watcher 回归用临时实际 .ets 验证 Windows notify 与 Drop；真实 ArkTS
源码刷新另由 D 驱动实测。强认证登记和活动工程范围证明仍待实现。

## 4. 合法 SDK 与 D1 / D2

用户已合法获取并解压到 D:\CodingTools\HarmonyOS\command-line-tools。
[官方独立命令行工具下载](https://developer.huawei.com/consumer/cn/download/command-line-tools-for-hmos)
获取需要 Huawei 登录及相应许可；本应用不代接受或重新分发 SDK。
在启动 ArkDock 的 Shell 设置 ARKDOCK_SDK，或在设备页选择完整 Command Line Tools 根目录。
本机组合为 Command Line Tools 26.0.0.851、SDK 26.0.0.105/API 26、Hvigor 6.26.8。
不要把 D:\CodingTools 整个工具目录当 SDK 根。

必须记录具体 SDK/构建工具/Previewer 版本、合法来源、官方命令和协议依据、文件 hash、
产物格式、帧/输入协议和能力。OpenHarmony 公开实现只参考，不能推定商业兼容。
tests/fixtures/harmony-app 是故意不可构建的只读发现 fixture，不用于原生验收。

PreviewBackend 已接通该具体配置：构建、启动、帧、点击、完整源码重建、停止、
超时、错误恢复、单工程串行/变更合并/旧任务旧帧拒绝/背压/失败旧画面提示。
仅 Stage 默认 product/target、pages/Index、360×640；重建重启会重置组件状态。
只为自己创建的进程树使用 Windows Job，不按进程名终止。

D 驱动必须实际断言：DevEco GUI 关闭 → 实际 ArkTS 构建退出 0 与产物/hash →
ArkDock 收到带 session/build/generation 的真实帧 → 点击改变实际组件 →
实际 .ets 修改后产生更新。也覆盖失败恢复和迟到拒绝。
tools/native-smoke.mjs 会使用 examples/native-counter 的实际计数器，临时信任自有示例，
用实际 WebView2 鼠标点击并检查真实 Previewer 组件日志/帧改变，再修改真实 .ets。
还故意触发编译失败、修复、撤销/重新授权和停止；最终恢复源码与原先信任。
DevEco GUI 必须关闭；驱动只检查，不终止它。
native-support.json 不参与门禁，protocol/native-profile.json 只作已实测版本摘要预检。

运行 D 驱动前，须按第 2 节在同一工作区启动 pnpm dev（带 SDK 和临时 debug 端口）。
缺少该实际应用时 gate 退出 2；断言/执行超时失败退出 1。
fetch/握手 5s、每步有期限、驱动总期限 15 分钟，抛错会执行清理；父 gate 不强杀驱动。
当前自托管 CI 同样需要可交互用户桌面与正在运行的该工作区开发应用，远端未执行。

## 5. 最终门禁

```powershell
pnpm run doctor
pnpm verify
pnpm verify:native
pnpm verify:release
```

已匹配本机工具链、运行真实开发应用时 doctor/base/native 应退出 0；
release 再次执行基础和实际原生驱动，全部 PASS 才调用 Tauri NSIS 打包。
包只以实际生成路径/摘要报告；debug exe 不是安装包，不自动推送或发布。
每次记录 PASS/FAIL/BLOCKED/SKIPPED、命令退出码、版本和证据。
不要推送/发布；报告和本地路径分享前脱敏。
