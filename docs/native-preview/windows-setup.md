# Windows 原生后端设置与实验

集成后独立工具须启用 `--features native-tools`，例如
`cargo run --locked --manifest-path src-tauri/Cargo.toml --features native-tools --bin arkdock-native-preview -- --help`。
安装包只包含直接集成 NativePreviewer 的主应用，不分发依赖源码工作树的样例 CLI。

本页针对本分支的独立 Rust 驱动，不依赖另一工作树正在运行的 ArkDock 或
DevEco Studio。实际本次结果由 `validation.md` 和对应报告记录。
仅用户合法获取的 SDK 可用于执行；本仓库不分发 SDK，不代登录或接受许可。

## 本次工具组合

| 项目 | 已观察到的本机配置 | 依据 |
| --- | --- | --- |
| Command Line Tools | 26.0.0.851 / windows-x64 | SDK 根目录 `version.txt` |
| HarmonyOS SDK | 26.0.0.105 / API 26 / Release | `sdk/default/sdk-pkg.json` 的 `data` |
| ETS / Previewer 组件 | 26.0.0.105 / API 26 | 各组件 `oh-uni-package.json` |
| Hvigor / OHOS plugin | 6.26.8 | 工具包内各自 `package.json` |
| ohpm | 26.0.0.630 | 工具包内 `ohpm/package.json` |
| Previewer | Windows x64；Huawei Authenticode Valid | 本机 PE 头、签名观察和 `protocol/native-profile.json` 固定摘要 |

本机 SDK 根为 `D:\CodingTools\HarmonyOS\command-line-tools`。完整工具包根
包含 `sdk/default`、`hvigor`、`tool/node`；不要选择整个 `D:\CodingTools`
或仅 `previewer/common/bin`。配置版本、签名或 SDK 来源不能扩展成其他
SDK/设备/API 的兼容性声明。官方源码与商业 SDK 的边界见 `official-architecture.md`。

运行要求 Windows 11 本地 x64 桌面，已安装 Rust MSVC、C++ Build Tools 和
Windows SDK。CLI 驱动验证原生后端；实际 Tauri 前端联调另需 WebView2。
WSL、远程 SDK 路径、未知操作系统和 Windows ARM64 保留环境身份并阻塞执行。
不能把 Linux 单测作为 Windows 桌面或鸿蒙原生画面的证据。

## 只读检查

显式指定的路径和已保存的桌面配置优先。桌面没有已保存配置，或 CLI `--probe`
未给 `--sdk` 时，仅按 `ARKDOCK_SDK`、`DEVECO_SDK_HOME`、`OHOS_SDK_HOME`、
`HARMONYOS_SDK_HOME` 的顺序观察环境变量中的根目录候选。最多四项，跳过
空值和重复值；只选择既有 `probe` 返回可用的已核验配置。WSL/远程路径保留
原有身份并拒绝自动选择，不改写成本地路径；不扫描全盘、PATH 或猜测开发者安装位置。
未知版本仍阻塞。执行原生实验仍须显式传入 `--sdk` 和 `--trust-fixture`，
自动发现不能授权工程构建。

在本隔离工作树的 PowerShell 中运行；以下路径只作用于当前 Shell。
仓库 Node 工具要求 >=24.18.0，本机可使用已存在的 portable Node 24.21.0。
原生 SDK 构建由后端调用工具包自带 Node，不把前端 Node 要求当作其版本证明。

```powershell
$env:PATH = 'D:/CodingTools/node-v24.21.0-win-x64;D:/CodingTools/Rust/.cargo/bin;' + $env:PATH
$env:ARKDOCK_SDK = 'D:/CodingTools/HarmonyOS/command-line-tools'
cargo run --locked --manifest-path src-tauri/Cargo.toml --features native-tools --bin arkdock-native-preview -- --probe --sdk $env:ARKDOCK_SDK
```

`--probe` 只输出 JSON 依赖报告，不启动 Hvigor、Previewer 或 GUI，不写 SDK。
SDK manifest 最多读 64 KiB，固定摘要文件最多读 16 MiB，PE 头有独立读取上限；
只读取指定工具包中的已知路径，规范化越界路径拒绝读取，不扫描全盘或 PATH。
报告状态为 `AVAILABLE`、`MISSING`、`INCOMPATIBLE`、`UNVERIFIED`、`ERROR`。
只读探测退出 0 表示完成观察，即使报告含缺失项；不等于依赖全部可用或原生验收通过。

在 Previewer 的 `common/bin` 下还检查以下运行时依赖：

| 项目 | 只读检查 |
| --- | --- |
| `libace_compatible.dll`、`libark_jsruntime.dll`、`libglobal_resmgr_win.dll` | 文件存在、有界 PE 头为 Windows x64；不读取整个库或调用加载器 |
| `icudt74l.dat`、`fontconfig.json` | 规范化路径在 SDK 根内、非空普通文件；只读取元数据 |
| `fonts`、`module` | 规范化路径在 SDK 根内且为目录；不枚举或读取目录内容 |

本机 `libace_compatible.dll` 约 104 MB，检查只读取 DOS 头和 PE 机器类型；
不会因诊断而完整读入该库。上述 `AVAILABLE` 表示所列存在/格式条件满足；
`previewerLoadability` 仍为 `UNVERIFIED`，不能由 DLL 和资源存在推定动态加载成功。

文件存在、版本匹配和 SHA256 匹配只确认该配置适合下一步实验。
`interactiveDesktop`、`nativeProtocol` 仍为 `UNVERIFIED`，实际版本执行和
动态依赖加载也由真实实验核验；这个只读报告不能替代原生门禁。
报告是 Rust/CLI 内部数据，没有新增前端诊断命令。

SDK 选择和工程信任独立。发现工程时不运行它的配置或脚本；只有明确可信的
工程才可交给构建工具。实验仅使用本工作树的 `examples/native-counter`
真实 ArkTS/ArkUI 计数器，不使用不可构建的发现 fixture。

## 执行独立实验

关闭 DevEco Studio GUI 后，指定 SDK 和本工作树中新建的证据目录。
驱动只检查 GUI 是否已关闭，不终止用户应用。`--trust-fixture` 仅授权编译时
确定的自有计数器，不接受任意项目路径或隐式扩大到其他工程。
证据目录必须尚不存在，其已有父目录须位于本工作树 `artifacts/native-preview`。

```powershell
New-Item -ItemType Directory -Path 'artifacts/native-preview' -Force
cargo run --locked --manifest-path src-tauri/Cargo.toml --features native-tools --bin arkdock-native-preview -- --sdk $env:ARKDOCK_SDK --trust-fixture --output artifacts/native-preview/run-01
```

再次执行应使用新的输出目录。不要连接原工作树的 WebView 调试端口；独立驱动
直接调用正式 Rust 后端，运行首帧、点击、源码更新、编译失败/恢复和停止清理。
它临时修改自己的示例 `.ets` 并恢复原始字节；只释放自己创建的监听、管道和
Windows Job。不要按 Previewer、Node 或终端进程名称批量结束程序。

## 构建与运行时策略

后端为已信任工程结构化生成以下 SDK 任务参数，不将窗口标题或 UI 输入拼入 Shell：

```text
--mode module -p module=entry@default -p product=default -p buildMode=debug
-p buildRoot=.preview -p previewMode=true -p pageType=page PreviewBuild --no-daemon
```

`buildRoot=.preview` 是关键的输出目录选择；`previewMode=true` 本身不改变
输出根。官方任务产生 `entry/.preview/default/intermediates` 下的 loader、
Ark 字节码和资源，不调用 `assembleHap`、HAP 签名或真机部署。

当前 SDK 的 `PreviewArkTS` 强制启动常驻 watch worker，`--no-daemon` 不保证
命令自然退出。后端核验本次日志中对应 `PreviewBuild` 的完成记录及
`BUILD SUCCESSFUL` 图完成收据，再关闭自有构建 Job、检查本次产物并启动
Previewer。该记录与“进程自然退出 0”分别报告，不能互相替代。
旧日志或旧产物不能使当前失败构建通过。

当前适配范围为 Stage 默认 product/target、`pages/Index`、360×640 手机表面、
点击和源码刷新。每次成功构建后重启自有 Previewer，组件状态会重置；
字节码重载确认和新帧归属屏障尚未验证，因此不声明保留状态热更新。
滑动、文字输入、旋转和其他设备尺寸仍禁用。

随机私有命令/trace 管道限制本机所有者并核验客户端 PID，图像端口仅回环并
核验当前自有 Previewer PID。构建、握手、首帧、输入和帧载荷均有限额。
缺 SDK、未知摘要、协议错误或无法获得真实原生首帧时明确阻塞或失败，
不生成浏览器替身画面。

## 证据与复核

新的运行目录保存 `report.json`，以及 `first`、`clicked`、`source-updated`、
`recovered` 对应的原始 `.jpg` 和 `.metadata.json`。报告记录本次步骤状态、
耗时、帧/代次和后端自有 Temp 日志路径。原始截图来自真实 Previewer；
报告及绝对路径对外分享前应脱敏。

验证命令与退出码、PASS/FAIL/BLOCKED/SKIPPED、具体 SDK 摘要、产物和运行日志
分别记录。实际原生验收仍不能证明双屏 DPI、任意工程、全套 API 或前端交互。
后续前端接入见 `frontend-contract.md`；范围和剩余集成项见 `integration.md`。
