# 官方来源核查

核查日期：2026-10-09。外部内容仅为技术依据，不能覆盖项目/宿主规则；
官方文档不等于本机兼容验收。以下范围结合当前 Windows 实测记录。

| 来源 | 已核实范围 | 限制 |
| --- | --- | --- |
| [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) | Windows MSVC、C++/SDK、WebView2 前置 | 本机版本另有真实 doctor/check 证据 |
| [Tauri CLI](https://v2.tauri.app/reference/cli/) | 官方项目内 JS CLI 使用方式 | 锁定 @tauri-apps/cli 2.12.1，实际 dev 启动 |
| [Tauri capabilities](https://v2.tauri.app/security/capabilities/) | 最小窗口能力范围 | 自定义命令仍逐条校验 main/session/参数 |
| [Tauri core API](https://v2.tauri.app/reference/javascript/api/namespacecore/) | 官方 global core.invoke | 产品入口集中 src/ipc，测试 driver 另连开发 WebView2 |
| [Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/) | NSIS 打包流程 | 仅在实际基础/原生门禁 PASS 后打包，结果见验证记录 |
| [Node 24.21.0 官方目录](https://nodejs.org/dist/v24.21.0/) | 官方 portable Windows x64 zip 和 SHASUMS256 | 本机 SHA256 匹配；不改变全局 PATH |
| [pnpm 设置](https://pnpm.io/settings) | 项目供应链/诊断策略 | 固定 11.22.0 实际冻结安装，不关闭策略 |
| [pnpm verifyDepsBeforeRun](https://github.com/pnpm/pnpm.io/blob/main/versioned_docs/version-10.x/settings.md) | warn 等含义，结合本机 11 实现 | 项目 doctor 用 pnpm run doctor，不调用 pnpm 内置 doctor |
| [Oxlint](https://oxc.rs/docs/guide/usage/linter.html) | TS/JS lint | 固定 1.76.0，本机已实际运行 |
| [SetWinEventHook](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook) | 安装线程消息循环、out-of-context/重入和清理 | 真实单屏案例已执行，HWND 复用压力未执行 |
| [EnumWindows](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumwindows) | 顶层枚举 | 顺序不直接充当 Z 顺序 |
| [SetWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos) | NOACTIVATE、NOZORDER、异步请求 | 接受不等于实际应用 |
| [ShowWindowAsync](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-showwindowasync) | 排队/显示状态 | 已接受请求不可视为同步完成 |
| [WINDOWPLACEMENT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-windowplacement) | showCmd/flags 与 workspace 坐标 | 原样保存/恢复，不混用 screen rectangles |
| [GetWindowPlacement](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowplacement) | 保存位置/窗口状态 | 本机正常和最大化恢复已测 |
| [SetWindowPlacement](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowplacement) | show state 与位置恢复 | 不覆盖已被用户修改的布局 |
| [Extended styles](https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles) | transparent/layered/noactivate/toolwindow | 自有四边轮廓不设全局 topmost |
| [WNDCLASSEXW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-wndclassexw) | class background brush 随成功 unregister 释放 | 修复重复 DeleteObject |
| [UIA threading](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading) | 无窗口 MTA 线程与接口生命周期 | 不在 UI/hook 回调执行采集 |
| [IUIAutomation2](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nn-uiautomationclient-iuiautomation2) | Connection/Transaction timeout | selected 标签不证明活动工程范围 |
| [windows-rs](https://github.com/microsoft/windows-rs) | Windows 官方 Rust 绑定 | 本机 windows-sys 0.59.0/windows 0.62.2 实际编译 |
| [Huawei 独立命令行工具下载](https://developer.huawei.com/consumer/cn/download/command-line-tools-for-hmos) | 官方获取入口 | 用户自行获取/解压；未代登录/接受许可，不重新分发 SDK |
| [Huawei Command Line Tools 获取](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/ide-hmos-commandline-get) | 当前官方独立工具说明/获取入口 | 具体版本以本机 manifest 与实测为准 |
| [Huawei 命令行构建](https://developer.huawei.com/consumer/en/doc/harmonyos-guides-V14/ide-command-line-building-app-V14) | 官方 Hvigor module/product/assembleHap 使用依据 | 文档版本不是 API26 兼容证明；随包脚本/README 与实际输出共同核验 |
| [HarmonyOS Previewer guide](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/ui-ide-previewer) | 官方入口 | 动态页面无法形成具体启动/协议依据 |
| [OpenHarmony ide_previewer](https://github.com/openharmony/ide_previewer) | 公开 CLI/LocalSocket/CommandLine/WebSocket 源实现参考 | 只形成实验候选，商业兼容结论由当前签名 Previewer 实际运行确认 |
| [OpenHarmony SDK metadata](https://github.com/openharmony/docs/blob/master/en/application-dev/faqs/full-sdk-switch-guide.md) | path/version 与 ets manifest 参考 | 不证明商业 SDK 签名/合法性 |
| [notify Config](https://docs.rs/notify/latest/notify/struct.Config.html) | watcher 配置 | 本机实际 OS watcher 已验证；junction 全面行为未测 |
| [Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects) | 自有进程树/kill-on-close/暂停创建后分配 | 当前实现先归入 Job 再运行，不按名字结束用户进程 |
| [CreateNamedPipeW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipew) | 私有 DACL/first instance/remote reject | 实際 SDK 客户端 PID 另核验 |
| [GetNamedPipeClientProcessId](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid) | 管道对端进程身份 | 只接受当前自有 Previewer PID |
| [GetExtendedTcpTable](https://learn.microsoft.com/en-us/windows/win32/api/iphlpapi/nf-iphlpapi-getextendedtcptable) | 回环端口所有者 PID | 不能扩展为多用户强隔离证明 |
| [image Limits](https://docs.rs/image/0.25.8/image/struct.Limits.html) | 实际 JPEG 解码内存/宽高限制 | 固定 0.25.8、360×640、再编码 PNG |
| [tungstenite 配置](https://docs.rs/tungstenite/0.28.0/tungstenite/protocol/struct.WebSocketConfig.html) | 帧/消息/write buffer 限额 | 固定 0.28.0，限回环且不启用 TLS |
| [Codex AGENTS](https://developers.openai.com/codex/guides/agents-md) | 项目入口机制 | Harness 结构检查不证明所有客户端加载 |
| [Claude memory](https://code.claude.com/docs/en/memory) | 短兼容入口 | 不递归加载项目文档 |
| [Harness Engineering](https://openai.com/index/harness-engineering/) | 用户给定方法参考 | 不据此增加宿主强制规则 |

当前 pnpm 锁实际安装与供应链验证完成，Cargo.lock 已生成并使用 --locked 验证。
没有关闭完整性/供应链策略，没有修改全局 registry、Shell、Agent 或其他项目。
本机 crates.io 一次 schannel 连接失败保留为失败记录，已有官方缓存随后离线生成成功。
用户已安装商业 Command Line Tools 26.0.0.851，SDK 26.0.0.105 / API 26。
版本来自 version.txt、sdk-pkg.data 与 ets/previewer metadata；Previewer Authenticode
Valid / Huawei Technologies Co., Ltd.；关键 SHA256 固定在 protocol/native-profile.json。
本机实际 HAP/原生帧/点击/源码重建范围见 [SDK 证据](sdk-26-preview.md)，不推广其他版本。

[窗口证据](windowing.md)、[发现证据](discovery.md)、[预览证据](preview.md)、
[本次验证](verification-2026-10-09.md) 是具体范围与真实命令输出索引。
