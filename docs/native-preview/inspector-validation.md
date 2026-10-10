# SDK 26 Inspector 原生验证

日期：2026-10-10（Asia/Shanghai）。实验只运行隔离工作树自有 `examples/native-counter`，使用既有随机私有管道、Previewer PID 校验、回环端口所有权与 kill-on-close Job。未操作用户运行中的 Previewer、其他窗口、聊天数据库或 DevEco GUI；未修改 SDK、全局环境或源码样例。

## 环境与命令

Windows x64；商业 Command Line Tools `26.0.0.851`，HarmonyOS SDK `26.0.0.105` / API 26，Hvigor `6.26.8`。固定二进制摘要继续由 [native-profile.json](../../protocol/native-profile.json) 准入。Previewer SHA256 为 `356baec698a3769a94d069d43848b163f82d5695c5745ee0f4800b77c7f1c05d`；本次 `modules.abc` 为 `181f1419d9da53b74119e96b721982ce80495a227394cc7e8cb0656ded24d76f`。每个运行目录有只读 `environment.json`，含工具版本及摘要。

实际执行：

```text
cargo run --manifest-path src-tauri/Cargo.toml --bin arkdock-inspector-probe --features native-tools -- --trust-fixture <本机SDK根> inspector-baseline-20261010
cargo run --manifest-path src-tauri/Cargo.toml --bin arkdock-inspector-probe --features native-tools -- --trust-fixture <本机SDK根> inspector-settled-20261010
cargo run --manifest-path src-tauri/Cargo.toml --bin arkdock-inspector-probe --features native-tools -- --trust-fixture <本机SDK根> inspector-one-read-baseline-20261010
cargo run --manifest-path src-tauri/Cargo.toml --bin arkdock-inspector-probe --features native-tools -- --trust-fixture <本机SDK根> inspector-bounded-drain-20261010
cargo run --manifest-path src-tauri/Cargo.toml --bin arkdock-inspector-probe --features native-tools -- --trust-fixture <本机SDK根> inspector-final-20261010
```

上述运行退出 0，原生协议结果 PASS。证据目录为 `artifacts/native-preview/<同名目录>/`；原始响应 `inspector-before.json`、`inspector-default.json`、`inspector-after.json`、`inspector-settled.json`；图像 `before.jpg` / `after.jpg` / `settled.jpg` 与独立 metadata。后续三次运行另外保存解析的 `*-tree.json` 和 12 次重复查询。原始构建/运行日志位于报告输出的自有 Temp/ArkDock-native 目录；最终运行已复制 `build.log` / `previewer.log` 到其证据目录。

## 实际请求与响应

经同一 owned commandPipe 发送以下 UTF-8 JSON，末尾单个 NUL：

```json
{"version":"1.0.1","type":"action","command":"inspector","args":{}}
```

`inspectorDefault` 使用相同结构和 type。商业命令均返回 `{version,command,result}`，result 是 JSON 字符串；Inspector 为 32608 bytes，default 为 42716 bytes。default 是默认属性集合，不能用来替代实时组件树。空字符串、`null`、空树替代文案和未知 schema 均按 BLOCKED 处理；本次商业二进制没有返回空树。

真实 root 为 `{"$type":"root","width":"360.000000","height":"640.000000","$resolution":"1.000000","$children":[...]}`。稳定 tree 含 5 个节点：

| 原生 ID | 类型 / 父 ID | 原生矩形 x,y,w,h | 调试位置 |
| --- | --- | --- | --- |
| 5 | Column / 空 | 0,0,360,640 | Index.ets(11:5) |
| 6 | Text / 5 | 28,206,304,30 | Index.ets(12:7) |
| 7 | Text / 5 | 99,260,163,49 | Index.ets(17:7) |
| 8 | Button / 5 | 27,333,306,56 | Index.ets(21:7) |
| 10 | Text / 5 | 90,413,181,21 | Index.ets(30:7) |

`$attrs` 实际有 `.id()` 值、content、字体、颜色、尺寸、visibility、opacity、clip、zIndex、布局、触摸等属性。原生 `$ID` 与 `.id('native-count')` 是不同标识。`$debugLine` 是 JSON 字符串，含 `$line` 和 `$packageName`；源码路径为 `entry/src/main/ets/pages/Index.ets`。本样例行号已与实际文件对应，其他工程的映射需单独核验；没有树级 source hash、page 标识、原子 frame ID 或可靠 paint order。

360×640 的原生 JPEG 中按钮边界观察为 x=27..333、y=333..389，与 tree 的 Button 矩形一致；中心 (180,361) 在按钮内。通过已验证 click 命令，日志出现 `ARKDOCK_NATIVE_COUNT=1`；稳定 JPEG 和后续组件树均显示 Count:1，树层级/ID 保持一致。

首次快速采样中，点击后 200 ms 获取的图像和 tree 仍为 Count:0，尽管稍后运行时日志已记录 Count:1。这是独立通道和渲染动画的真实顺序限制；后续等待稳定画面、连续消费 mailbox 的运行在 sequence 7 验证 Count:1。固定延时不是通用完成屏障，产品仍将 `exactFrameMatch=false` 并在新帧时失效缓存。

## 线程与响应耗时

官方源码中 rich 命令线程分派 action，`AceAbility::GetJSONTree` 同步投递 UI task 收集 tree。商业日志中 CommandLine / Inspector start/end 在命令线程，JS click 日志在另一线程；本样例 Inspector start/end 约 16–18 ms。日志与源码一致，但未对商业 DLL 做私有反汇编；不能声称已验证所有 UI 线程内部实现。

ArkDock 的请求进入 bounded channel，帧 worker 发送/读取，不运行在 Tauri 主线程或 Windows 事件线程。响应 wire 没有 request ID，因此仅单查询在途。2 秒超时后禁止当前运行时继续 Inspector 查询；重启原生运行时才重建关联。

实际发现 pipe 每 tick 只读取 8192 bytes，32608-byte tree 被拆到多个帧循环，造成约 100 ms 的附加等待。改为每 tick 最多 8 个 buffer（64 KiB）后保持有界并缩短响应消费。

| 比较 | 样本数 | p50 / p95（nearest-rank） | 指标边界 |
| --- | --- | --- | --- |
| one-read baseline 12 次固定 tree | 12 | 101 / 101 ms | 宿主写入查询至完整响应消费 |
| bounded-drain 12 次固定 tree | 12 | 34 / 35 ms | 同工程/SDK；不同新进程 |
| 最终版本重复验证 12 次固定 tree | 12 | 34 / 50 ms | 已校验商业响应 version=1.0.1 |

基线样本 `[101,101,101,100,101,101,100,101,100,88,101,101]`；优化样本 `[33,34,34,34,35,34,33,35,34,33,35,34]`；最终复验样本 `[34,50,33,35,34,33,33,34,35,34,34,34]`。最终 `query-latency-comparison.json` 保存计算数据。最终运行初帧 679 ms、实际 PreviewBuild 9270 ms，普通查询没有新构建或运行时重启。这些是 Inspector 查询耗时，**不是**用户点击到 WebView2 显示反馈的 p50/p95。未在此实验量化 CPU、WebView 解码、绘制、Windows 桌面覆盖层或所有布局场景。

## 固定源码证据

只读取官方不可变快照，未构建/执行下载代码。2026-10-10 06:45:13–15 UTC 抓取，原始文件与下载摘要在 `artifacts/native-preview/inspector-source-20261010/sources.json`。

| 来源 | SHA256 |
| --- | --- |
| [CommandLineFactory.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLineFactory.cpp#L30) | `c862a0f52377c15d04d2783e0f099a598e6c6c849e83986fdca6b1e00fb883da` |
| [CommandLine.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/cli/CommandLine.cpp#L1167) | `c4037e99107d087e74b322ad917dc46a850f3dc0aa8d27b92b297488f19d0960` |
| [JsAppImpl.cpp](https://github.com/openharmony/ide_previewer/blob/8b86305d7853d96aee92aeccab2c2daafa0d2f5b/jsapp/rich/JsAppImpl.cpp#L102) | `0c4065d5bc6c560dcf309ea2343e54f24f49006a4f9432ba53ac8722ff55d2e1` |
| [ace_ability.cpp](https://github.com/openharmony/arkui_ace_engine/blob/21b150e7bb1d5308846f8e5be8ef0482fb433315/adapter/preview/entrance/ace_ability.cpp#L545) | `7cf2c8b94a4124dafce8f1d04a4fdeb755c98719a753181386e38d7d1dc97778` |

该 OpenHarmony 快照不是商业 SDK manifest；商业功能依据上面的实际二进制执行，而非 master 源码支持。

## 门禁边界

`cargo test --manifest-path src-tauri/Cargo.toml --lib inspector::tests` 退出 0，18 个匹配测试通过，其中 3 个新增 Inspector 测试是 synthetic parser/identity/clip 回归，不能代替原生证明。原生构建、私有管道、命令、tree、静态矩形和点击后数据更新 PASS。任意复杂裁切/叠放/滚动命中、动态画面的原子 tree/frame 对应、桌面 Inspector 模式点击隔离、Agent 当前会话提交与完整 Windows GUI 回归在本模块实验中 SKIPPED；由各自集成门禁另报。其他 SDK / WSL / remote 原生 Inspector BLOCKED。
