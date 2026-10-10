# ArkDock

## License

ArkDock's own source code is licensed under [Apache License 2.0](LICENSE). It preserves copyright and license notices and grants relevant contributor patents. It permits commercial use, modification, and redistribution; it cannot prohibit lawful uses or guarantee that others will not misuse the code. Third-party components retain their separate licenses in `src-tauri/resources/third-party-notices.txt`. The HarmonyOS SDK and Previewer are not covered by this license and are not distributed here.

鸿蒙外置贴边预览伴侣。Windows 11 独立桌面窗口，跟随普通应用或终端，
以证据关联实际工程。Rust 管理系统能力，React 显示后端状态。

**当前为有实测范围的桌面预览实现。** 本次已在 Windows 11 原生环境
安装锁定依赖、编译 Rust 与桌面、验证单屏外置联动及浅色/深色/系统主题。
用户合法安装的 HarmonyOS 26.0.0.105 / API 26 已核验，真实 ArkTS 示例实现
无 DevEco GUI 构建、实际 Previewer 画面、点击、源码重建与失败恢复。
仅匹配当前工具包摘要、Stage 默认目标、pages/Index、360×640；其他版本保持阻塞。
浏览器调试显示缺少桌面桥接，不生成 ArkUI 仿制画面。
双屏、高权限与 HWND 压力案例仍未实测，见 [验收与证据](docs/evidence/compatibility.md)。

## 安装与手动验收

Windows 11 x64 普通用户使用交付的 NSIS `.exe` 安装包，从开始菜单或安装目录
启动独立 ArkDock 窗口。运行安装版无需 Rust、Node、pnpm、Vite 或开发服务器；
桌面界面需要 WebView2，真实原生预览另需用户合法安装的完整 HarmonyOS Command Line Tools。
按 [Windows 安装版验收指南](docs/testing/windows-acceptance.md) 检查窗口联动、工程发现与信任、
SDK 配置、真实首帧/点击/刷新、错误恢复和诊断。实际安装包路径、摘要与本轮结果以交付记录为准。
本轮 0.1.0 为未签名测试构建。NSIS 已实际安装、启动、重启及卸载，缺 SDK 可正常显示阻塞。
MSI 为系统级安装包，未执行安装测试；普通用户优先 NSIS。
完整鼠标贴边与 React 原生画面验收尚未通过，见 [本轮交付记录](docs/evidence/integration-delivery-2026-10-10.md)。

## 命令

以下为源码开发与门禁命令。

Node ≥24.18.0、pnpm 11.22.0。Windows 桌面还需要 Rust MSVC 工具链、
Tauri 2 CLI、WebView2 与官方 Windows 构建依赖，见 [原生手册](docs/runbooks/windows-native-preview.md)。

pnpm 11 内置 `doctor` 会遮蔽同名项目脚本，项目诊断须使用 `pnpm run doctor`。

```sh
pnpm install --frozen-lockfile
pnpm run doctor
pnpm test:portable
pnpm verify
pnpm dev
pnpm desktop:build
pnpm verify:native
pnpm verify:release
```

不需要 npm 依赖的可执行部分：

```sh
node --test --test-isolation=none tests/portable/*.test.mjs
node tools/verify-harness.mjs
node tools/generate-ipc.mjs --check
node tools/check-boundaries.mjs
node tools/doctor.mjs --sdk /your/explicit/sdk/root
```

`pnpm dev:ui` 仅开发 ArkDock 自身界面，不验证原生能力。
工程信任必须另行确认。`pnpm run doctor` 只读检查，不执行工程脚本或未知 SDK 二进制。
统一门禁约定 0=PASS，1=FAIL，2=BLOCKED；缺环境不能按 SKIPPED 当作通过。

`pnpm-lock.yaml` 已包含精确 npm 依赖与完整选定依赖图的 integrity 元数据。
本机冻结安装与供应链校验已完成，src-tauri/Cargo.lock 已生成并用于锁定验证。
项目内官方 Tauri CLI 无需全局安装；本机 portable Node/pnpm 的进程 PATH 见原生手册。

## 用户流程

启动独立窗口，按住“拖动贴边”靠近目标右侧，候选文字和四边穿透轮廓提示；
松开绑定后进行证据解析。UIA 选中标签元数据、登记与标题仍不足以证明活动工程，
须显式选择；唯一可靠来源的自动决策规则已有实现，但默认强来源未接入。
无可选本地工程时选择实际目录；关联与执行信任分别确认。
指定完整 Command Line Tools 根目录后，匹配配置才开放真实构建与点击。
源码刷新执行完整重建/重启，组件状态重置；旋转、滑动、文字和保留状态热重载禁用。

## 工程地图

- [AGENTS.md](AGENTS.md)：简短入口与任务路由。
- [产品验收](docs/product.md)、[架构](docs/architecture.md)、[状态/下一步](docs/status.md)。
- [会话登记辅助命令](docs/runbooks/session-registration.md)：可撤销、弱候选登记。
- [官方来源](docs/evidence/sources.md)、[质量门禁](docs/harness/quality.md)。

原生样例必须用用户合法 SDK 的实际工程验证。`tests/fixtures/harmony-app`
含真实 ArkTS 控件源码，但故意阻止执行构建，仅用于只读发现测试，不能用它证明 D1/D2。
真实验收工程为 [examples/native-counter](examples/native-counter)，
产物、截图、命令和包状态见 [验证记录](docs/evidence/verification-2026-10-09.md)。

界面采用蓝色玻璃卡片、侧边导航和设备空状态；宽窗并排、窄窗上下排布。
顶部快捷切换或设置页选择浅色/深色/跟随系统，偏好保存在本地。
开发 WebView2 的实际窗口/主题驱动为 `pnpm verify:windows` / `pnpm verify:ui`，
需要按原生手册临时开启调试端口。这两者不替代鸿蒙原生验收。
