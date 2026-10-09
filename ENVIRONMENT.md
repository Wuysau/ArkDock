# ArkDock 开发环境

当前代码：Windows 11 独立桌面应用，Tauri 2 / Rust、React 18 / TypeScript 6 / Vite 6。
桌面窗口能力仅支持 Windows 本地；浏览器调试不具备窗口绑定或鸿蒙原生渲染能力。

## 版本与依赖

- Node >=24.18.0；本次使用官方 portable Node 24.21.0。系统默认 24.13.0 不满足下限。
- pnpm 11.22.0；前端版本由 package.json 与 pnpm-lock.yaml 固定。
- Rust / Cargo 1.94.1，Windows MSVC 工具链；Rust 依赖由 src-tauri/Cargo.lock 固定。
- Tauri CLI 2.12.1，项目内安装；Windows C++ 工具链、Windows SDK 和 WebView2 必需。
- 本机：Windows 11 x64 26300、MSVC 14.44.35207、Windows SDK 10.0.26100.0、WebView2 154。
- 本机单屏 192 DPI；没有不同 DPI 双屏的实测结论。

## 启动与检查

本机已有 portable 工具时，仅在当前 PowerShell 设置：

```powershell
$env:PATH = 'D:/CodingTools/node-v24.21.0-win-x64;D:/CodingTools/ArkDock-pnpm/node_modules/.bin;' + $env:PATH
$env:ARKDOCK_SDK = 'D:/CodingTools/HarmonyOS/command-line-tools'
```

```powershell
pnpm install --frozen-lockfile
pnpm dev:ui
pnpm dev
pnpm typecheck
pnpm lint
pnpm test:portable
pnpm build
```

独立开发测试入口：pnpm dev:demo、pnpm build:demo、pnpm test:ui。
测试画面标识为“UI 演示 · 非原生渲染”，正式 pnpm build 不包含 fixtures。
Windows 桌面检查：pnpm verify:ui、pnpm verify:windows；需要运行中的开发 ArkDock。
自动驱动只在本次开发进程设置 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223。
不应将该调试设置写入全局环境或正式应用配置。

## 原生 Previewer

合法安装的 Command Line Tools 26.0.0.851 / HarmonyOS SDK 26.0.0.105（API 26）、
Hvigor 6.26.8；配置 ARKDOCK_SDK 为 Command Line Tools 根目录。
当前已核验适配器只支持 Stage 默认 target、pages/Index、360×640 与点击输入。
滑动、文字、旋转、页面/设备/应用主题切换、模拟器和真机尚未接入。
pnpm verify:native 每次必须运行真实构建、帧、点击、源码刷新与失败恢复断言。
SDK 商业工具不纳入仓库；缺 SDK 或协议不能以测试画面通过原生门禁。

## 本次代码检查点

基础 pnpm verify、Rust desktop 71 项、便携回归 31 项均退出 0。
真实 verify:ui（含无边框鼠标缩放）与 verify:native（SDK 帧/点击/更新/失败恢复）退出 0。
verify:windows 压力回归退出 1：最大化目标并排布局/迟到恢复仍需定位。
单屏基础联动通过不能覆盖该失败或不同 DPI 双屏。原生吸附目前即时定位，尚无缓动。
本次提交仅保存源码、配置、测试/工具与环境说明；设计、参考图片和 Harness 文档保留本地。
全量 verify 含文档引用检查，需要未提交的本地文档；代码检查可单独使用上面的命令。
