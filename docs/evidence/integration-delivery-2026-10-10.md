# 2026-10-10 前后端集成与 Windows 测试包交付

## Git 与安全边界

初始前端：feat/joint-dock-layout，4ebb8282a0df4ddfdb863933b8726ffadf472e03，
D:/Projects/Rust/ArkDock；源码干净，AGENTS/CLAUDE/README 和 docs 尚未跟踪。
后端：feat/native-preview-backend，7cf47d0a199f4100dff4c53cb3da72edddda1278，
D:/Projects/Rust/arkdock-native-preview；工作树干净。
共同祖先：aab0f5e2c579e8759bb3497d642d8836f3094af1。
main：c090418。无远程、无原 dev；从 main 新建 dev 与 backup/dev-before-integration-20261010。
隔离工作树：D:/Projects/Rust/arkdock-integration-20261010；集成分支 integration/arkdock-frontend-native。

前端正常 merge 保留历史；后端合并提交 071d865。两源 HEAD 均为其祖先（两次 merge-base --is-ancestor 退出 0）。
只有生成协议文件摘要冲突，从统一 protocol/model.json 重新生成。前端 UI/样式/共同窗口布局保留。
主工作树 Harness/设计文档仅核对复制到新集成工作树，没有提交、覆盖或删除来源原件。
不推送、不发布、不删除任何 worktree。最终 dev/构建提交以交付目录 manifest.json 为准，避免文档自引用 SHA。

## 实施与审查

- 新后端 PreviewBuild 及 JPEG40/二进制会话契约接入现有 Tauri/React PNG 兼容入口。
- execution_gate 输出独立 phase/trusted 决策；SDK 不可用关闭资源但保留有效工程授权，撤销/配置指纹失效仍拒绝。
- 原生驱动适配未信任关联无执行会话；增加实际 ArrayBuffer/JPEG/版本/旧 scope/恢复 PNG 断言，未以伪画面通过。
- UI 边缘缩放驱动增加几何与焦点诊断，保留原位置和全部尺寸断言。
- 新增人工指南；采用项目内官方 NSIS/WiX 缓存、当前用户 NSIS、静态 VC runtime、WebView2 bootstrapper。
- 独立最终只读审查没有额外可行动代码缺陷；桌面门禁失败单独记录。

## 环境与真实命令

Windows 11 x64 10.0.26300，单屏 2880×1800、192 DPI。Node 24.21.0、pnpm 11.22.0、
Rust/Cargo 1.94.1 x86_64-pc-windows-msvc；MSVC 14.44.35207、Windows SDK 10.0.26100.0、WebView2 154.0.4258.62。
用户合法 SDK：Command Line Tools 26.0.0.851，HarmonyOS SDK/Previewer 26.0.0.105/API 26、Hvigor 6.26.8。
仅进程 PATH/ARKDOCK_SDK/临时开发 debug 参数；未改全局环境、Agent 配置、Shell profile、防火墙或 SDK。

以下未写绝对路径的证据位于集成工作树 artifacts/delivery-2026-10-10/。

| 命令/范围 | 状态 | 退出码与证据 |
| --- | --- | --- |
| pnpm install --frozen-lockfile | PASS | 0；锁定依赖与供应链检查，无版本升级 |
| 首次 pnpm verify（安装仍未结束） | BLOCKED | 2；base-verify.log，前端依赖未就绪；后续完整重跑 |
| pnpm verify | PASS | 0；base-verify-final.log / artifacts/verify.json，37 Node、102 核心 + 8 CLI + 2 CLI + 1 watcher + 6 持久化 |
| cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop --lib | PASS | 0；desktop-tests.log，108 项 |
| 信任门禁新增回归初次编译 | FAIL（开发过程） | 101；trust-gate-red.log，旧门禁缺少独立 trust 决策类型；修复后库测试通过 |
| pnpm verify:ui | FAIL（7 组 PASS） | 1；ui-smoke-ready.log / ui/ui-smoke.json；主题、尺寸、键盘、面板、最小化通过，鼠标角落缩放失败 |
| pnpm verify:joint | FAIL | 1；joint-smoke.log / joint/joint-smoke.json；目标绑定超时，初始未绑定 resize 检查通过 |
| pnpm verify:native | FAIL | 1；native-gate.log / artifacts/native-gate.json / native-drag.json；SDK 摘要通过，拖动/关联前置未通过，React 原生断言未执行 |
| cargo run --locked --manifest-path src-tauri/Cargo.toml --bin arkdock-native-preview -- --sdk <合法根> --trust-fixture --output artifacts/native-preview/integration-20261010 | PASS | 0；native-backend-cli.log / artifacts/native-preview/integration-20261010/report.json，真实 T1–T7 |
| 双屏跨 DPI | BLOCKED | 只有单屏，几何单测不能替代 |
| 后续占用鼠标的 GUI 门禁 | SKIPPED | 用户选择先生成安装包，暂不提供空闲可见桌面 |

独立后端 T1–T7 包括真实构建/首帧、计数区域点击变化、.ets 文字区域更新、编译错误/恢复及自有进程停止。
不能作为 React 显示/点击映射或安装包原生预览通过的替代证据。原生源码已由 finally 恢复，git diff examples/native-counter 为空。
首次 UI 驱动在 dev 编译时两次端口未就绪，日志保留为 ui-before-launch.log、ui-smoke.log；不计产品通过。

## 桌面问题与边界

native-drag.json 显示拖动前后矩形未变化、outline 为空、前台 HWND 转向其他窗口，higher 中有覆盖区域。
现有证据无法确定边缘命中或焦点竞争的全部根因，不修改安全逻辑强行绑定。
复核时让自有目标和 ArkDock 边缘可见，按人工指南检查 200% DPI 下角落缩放、贴边与 React 原生流程。
默认强活动工程来源、键盘/IME、滑动、旋转、页面/设备切换、状态保留热更新仍未实现；刷新重启并重置状态。

## 软件产物

最终 dev 执行 Windows Release 构建；包与 SHA-256/字节大小/签名/构建提交保存于
artifacts/delivery-2026-10-10/manifest.json、SHA256SUMS.txt 和 release-build.log。
实际产物：NSIS ArkDock_0.1.0_x64-setup.exe 与 MSI ArkDock_0.1.0_x64_en-US.msi。
NSIS 专用目录安装、生产前端/Rust snapshot IPC、无 SDK 正常提示、正常关闭、无调试参数重启、卸载 PASS，
证据 installer-smoke.json / installed-state.json / installed-release.png；原有原生配置逐文件摘要保持不变。
MSI File 表仅主 exe 和三个许可资源；ALLUSERS=1，系统安装/卸载 SKIPPED，普通用户优先 NSIS。
PE AMD64、版本 0.1.0、签名 NotSigned；dumpbin 依赖为 Windows 系统/UCRT，无直接 VC runtime DLL 导入。
仅交付未签名测试包。完整 verify:release 未通过原生桌面前置，不能声明正式发布验收 PASS。
用户无需 Node/Rust/Vite；WebView2 Runtime 必需，缺失时安装器使用官方 bootstrapper（需要网络）。
不分发 Huawei SDK/Previewer，不把硬编码工作树的样例 CLI 当产品 sidecar。
安装/启动/卸载的实际状态以 installer-smoke.json 为准；未执行项不得写 PASS。

打包修复：Tauri 会自动分发 Cargo src/bin，第一次包曾包含开发 CLI。新增 native-tools 仅控制两 CLI 目标，
正式桌面 NativePreviewer 库不受该 feature 控制；verify 显式启用工具，全部 CLI 测试仍运行。
installer-exclusion-red.log 用旧安装包实际触发“不得安装开发工具”断言退出 1；app-only 重建后同一安装断言退出 0。
最终基础 app-only-verify.log 退出 0；desktop,native-tools --all-targets 125 项 app-only-desktop-tests.log 退出 0。
首次安装验证的 Tauri bundle tag 假设及许可证 LF/CRLF 差异失败记录保留；最终校验允许已核对的三个 bundle tag 字节，
其余 exe 字节严格相同，许可资源严格逐文件摘要相同。.gitattributes 防止更改上游许可证字节。

## 官方工具来源

固定 Tauri CLI 2.12.1 官方源码与本地 schema 核对 useLocalToolsDir；缓存 src-tauri/target/.tauri。
NSIS 3.11 与 WiX 3.14.1 归档下载到 toolchain，摘要符合固定源码常量。
NSIS v3.11@bytedminiprogram-ide 标记也出现在已核验官方归档，原缓存摘要与该归档一致；不据标记认定被改动。
来源：[Tauri Windows installers](https://v2.tauri.app/distribute/windows-installer/)、
[固定 CLI 源码](https://github.com/tauri-apps/tauri/tree/tauri-cli-v2.12.1)。
SDK 按现有安装许可使用、不重新分发；第三方许可材料和 MPL 源码归档随交付提供。

补充检查：pre-dev-verify.log 首次因原样 vendor COPYRIGHT.html 的格式约定失败退出 1；改为资源数据 .html.txt，保留原内容。pre-dev-verify-final.log 完整重跑退出 0。正式打包包装对 Cargo 传 --locked。

## Apache-2.0 与 GitHub 发布准备

Apache-2.0 的完整官方文本提交为根 `LICENSE`，标注 Copyright 2026 Wuysau；`package.json` 与 `src-tauri/Cargo.toml` 声明 SPDX `Apache-2.0`。仓库README解释适用范围、商业许可、第三方分离许可和SDK不包含。Apache许可文本逐字匹配官网正文并含该版权头。

`pnpm verify` 在许可变更后退出 0；LICENSE作为 `resources/LICENSE` 同步放进NSIS/MSI。许可证版Release `pnpm desktop:build`退出0。NSIS私有目录安装后，程序、三份第三方许可、项目LICENSE的摘要相符；现有配置摘要未变化。

许可证版WebView2启动门禁BLOCKED。交付Smoke发现已有ArkDock.exe属于 `D:/Projects/测试/ArkDock`，所以先前曾尝试启动的第二实例返回0x8007139F / exit 101。该工作区未被关闭/修改； smoke脚本现预检并在已有 ArkDock 进程时BLOCKED。旧的无LICENSE版启动PASS不能覆盖许可新版的窗口复测。

GitHub：账户 Wuysau 下 Wuysau/ArkDock不存在（仓库API 404）；本地没有remote；本机GitHub CLI凭据无效；Playwright浏览器在GitHub登录页。没有推送。需要用户创建空公开仓库、在本机gh重新登录后继续。完整Git历史的提交邮箱不是GitHub noreply格式；公开推送会公开提交对象里的作者邮箱，目前保留历史未重写。
