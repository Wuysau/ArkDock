# HarmonyOS 26 Windows SDK 实测配置

2026-10-09，用户自行获取并解压 Command Line Tools 到
`D:\CodingTools\HarmonyOS\command-line-tools`；ArkDock 不重新分发其二进制。
本次没有代登录账号、接受协议或启动 DevEco GUI。

## 具体工具

- Command Line Tools 26.0.0.851（windows-x64）。
- sdk/default/sdk-pkg.json 的 data：26.0.0.105 / API 26 / Release。
- ets、previewer oh-uni-package.json：26.0.0.105；HMS 组件随商业工具包提供。
- Hvigor/hvigor-ohos-plugin 6.26.8，ohpm 26.0.0.630；随包 Node v24.14.1。
- Previewer 位于 sdk/default/openharmony/previewer/common/bin/Previewer.exe。
  Authenticode Valid，签署者 Huawei Technologies Co., Ltd.；VersionInfo 无产品版本。
  SHA256 和实际构建入口的指纹见 [固定配置](../../protocol/native-profile.json)。

## 原始实验与正式驱动分开

`Previewer.exe --help` 实际退出 11，缺 -j；`-h` 实际退出 0，日志帮助输出不完整。
证据 artifacts/previewer-help.*.log、previewer-h.*.log。
公开 [OpenHarmony 源码](https://github.com/openharmony/ide_previewer) 只形成实验候选；
所有下述兼容结论来自当前签名二进制的实际执行，不能扩展到其他商业 SDK。

独立真实工程 [native-counter](../../examples/native-counter) 使用 ArkTS/ArkUI。
`assembleHap --mode module -p module=entry@default -p product=default
-p buildMode=debug --no-daemon` 实际退出 0，生成
entry/build/default/outputs/default/entry-default-unsigned.hap 及真实 modules.abc。
未签名 HAP 可用于本次 Previewer，不声明可安装到真机。
`PreviewBuild -p pageType=page` 实际失败，缺预览专用资源配置；没有修改 SDK 脚本绕过。
适配器使用已验证的完整 HAP 构建产物，源码更新完整重建并重启运行时，状态会重置。

实际启动参数由 src-tauri/src/preview/windows.rs 结构化生成：
-j loader_out/default/ets，-arp res/default，-ljPath loader/default/loader.json；
-pm Stage，-av ACE_2_0，-url pages/Index，-device phone，-or/-cr 360 640，-sd 160。
随机私有 -s/-ts 管道、-lws 本地端口和 -sid 令牌。
当前画面实测为 40 字节大端头 + JPEG，magic 12345678、两组宽高、保留字段全零。
Rust 限制消息/尺寸并解码 JPEG 为 PNG，再通过既有 session/build/generation 帧 IPC 展示。
MousePress/MouseRelease 使用 1.0.1 的 action JSON + NUL 结束，实际组件响应。

独立 probe 的实际 JPEG、日志只是协议实验；D1/D2 的成功必须由
`pnpm verify:native` 本次执行 [实际应用驱动](../../tools/native-smoke.mjs) 确认。
驱动检查 GUI 关闭、实际 ArkDock 图像、真实组件 console 计数、帧改变、实际 .ets 修改、
新构建/代次、失败旧画面与恢复，以及仅自有 Previewer Job 退出。
它不使用 tests/fixtures/harmony-app，也不读取历史 PASS 让门禁通过。

## 范围与安全

仅已指纹匹配的 Windows 商业 SDK 配置、Stage 默认目标与 pages/Index 页面通过此路径。
点击/完整源码刷新；滑动、文字输入、旋转、保留状态热重载禁用。
不是全套鸿蒙设备/API 兼容证明，其他版本必须重新实测。
构建异步、120s 超时；握手/首帧 15s；输入队列 16，最新帧邮箱背压，日志每次最多 16MiB。
进程在挂入 kill-on-close Job 后才恢复执行；私有管道限所有者和本机，核验客户端 PID；
图像端口核验 Previewer PID 和 127.0.0.1。停止/切换不按名称终止外部进程。
日志当前保留在系统 Temp/ArkDock-native 的自有随机目录；未提供自动留存清理 UI。

官方 wrapper 在自有隔离 HVIGOR_USER_HOME 安装 pnpm 10.28.2 时出现 npm 的高严重度 audit 提示；
未擅自执行 audit fix/升级 SDK 内部包。供应链提示与前端 pnpm 11 锁校验分别记录。
