# 原生后端验证记录

2026-10-09，Windows 本地隔离工作树，基线 `aab0f5e`。本页记录实际执行，
不引用另一工作树的历史 PASS 作为本次验收。最终独立后端验收为
`artifacts/native-preview/run-20261009-6/report.json`，退出 0。
报告 SHA256：`d2b0e2413e998a55d2b6cdf2521da0aede159efa0bfc79b1f30f113fb42203c8`。

## 环境与来源

- Windows 11 家庭版 中文版，`10.0.26300`，x64，可访问交互桌面。
- 用户已安装的 Command Line Tools `26.0.0.851`；HarmonyOS SDK
  `26.0.0.105` / API 26；Hvigor/OHOS plugin `6.26.8`；SDK Node `24.14.1`。
- Previewer 为工具包内签名二进制；本分支固定的三项摘要见
  [native-profile.json](../../protocol/native-profile.json)。不重新分发 SDK。
- 仓库检查使用 Node `24.21.0`、pnpm `11.22.0`、Rust/Cargo `1.94.1`、
  Windows MSVC。未另测 Rust 1.85 MSRV、其他 Windows/SDK 或 WSL。
- 只读依赖清单：`artifacts/native-preview/environment.json`。
  文件存在/架构/版本与实际动态加载分开；清单仍将运行条件标为 UNVERIFIED，
  实际加载/首帧由下表证明。
- 自动发现实测：仅在本次命令进程设置 `DEVECO_SDK_HOME` 为工具包的 `sdk`
  子目录，`--probe` 正确解析完整合法工具包根，没有启动 SDK；
  `artifacts/native-preview/environment-auto.json`。签名和 Windows 清单为
  `artifacts/native-preview/platform-sdk.json`，Previewer Authenticode Valid。

## 最终真实实验

构建了本工作树 `examples/native-counter` 的实际 ArkTS 源码；不是测试替身。
驱动调用正式 `NativePreviewer`、OS watcher、代次调度、原始 JPEG 接收和输入。
使用 `PreviewBuild` 专用产物，没有打包 HAP，没有打开 DevEco GUI。

| ID | 状态 | 实际断言及本地证据 |
| --- | --- | --- |
| T1 | PASS | 交互桌面可访问；两次 tasklist 精确查询均退出 0，DevEco GUI 不存在 |
| T2 | PASS | 构建图回执、非空 modules.abc/摘要、自有 Previewer 和有效 360×640 首帧；`first.jpg` / metadata |
| T3 | PASS | 实际 `COUNT=1` 日志和计数文字区域像素变化；人工查看 `clicked.jpg` 确认 Count:1 |
| T4 | PASS | 实际 `.ets` 保存由 OS watcher 驱动新代次，SECOND 日志和源码标签区域变化；人工查看 `source-updated.jpg` 为 SECOND |
| T5 | PASS | 故意无效 ArkTS 编译失败，最后成功帧/显示代次保留且 stale；修复后 `recovered.jpg` 和新代次恢复 |
| T6 | PASS | 原始源码字节恢复、监听释放、停止后无帧；持有三个自有 Previewer 的真实同步句柄并确认已结束 |
| T7 | PASS | 记录实际进程/构建/首帧/输入/保存反馈计时、JPEG 字节和邮箱计数；没有填模拟性能 |

上述图片、metadata 和报告位于 `artifacts/native-preview/run-20261009-6/`。
日志关联 workspace/session/build/generation，报告保留自有 Temp/ArkDock-native
目录的实际日志路径。T6 独立确认的是 Previewer 句柄；构建子树由暂停创建后
加入的 Windows kill-on-close Job 清理，没有另做每个 Hvigor 后代的句柄枚举证明。
不将这次有限生命周期检查扩展为长时泄漏压力证明。

## 实测性能条件

这是一次运行，SDK 文件与 Hvigor 缓存已存在，三个 Previewer 都是新进程。
首次构建 11,764 ms；初次进程启动到有效首帧 758 ms；含调度/构建/首帧的
T2 为 12,880 ms。后续有效构建为 7,438 / 6,797 ms，首帧 674 / 690 ms。
点击到实际计数区域新画面 415 ms；源码最后写入到可确认 SECOND 画面
8,768 ms。单帧 JPEG 18,785–19,419 字节。

T7 采样的是最后成功构建流：received=1、bytesReceived=19,419、dropped=0、
rejected=0、bufferedBytes=0，任务数 4。静态页面按变化产帧，这些计数不能
换算为通用 FPS。WebView 解码/Canvas 绘制耗时和长期帧率为 SKIPPED，未测量。
本任务没有达到瞬时刷新或状态保留；预览增量构建后重启会把 Count 重置为 0。

## 命令与门禁

命令在本工作树执行，原始输出在 `artifacts/native-preview/`。PATH 仅对当前
命令进程选择以上 Node/pnpm；没有修改全局环境、SDK、Shell 或其他项目。

| 命令 | 退出码/状态 | 证据 |
| --- | --- | --- |
| `pnpm install --frozen-lockfile` | 0 / PASS | `pnpm-install.log`；锁定依赖，锁文件未变 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` | 0 / PASS | `fmt-check.log` |
| `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --features desktop -- -D warnings` | 0 / PASS | 最终 `base-verify-final.log` / `artifacts/verify.json` 的 windows-desktop-clippy；此前 `clippy.log` |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --all-targets --features desktop` | 0 / PASS | `rust-tests.log`（111 项）；默认特征 106 项另见基础门禁 |
| `pnpm test:portable` | 0 / PASS（31 项） | `portable.log`；包括共享契约与二进制 IPC 边界 |
| `pnpm typecheck` | 0 / PASS | `typecheck.log` |
| `pnpm exec oxlint src tools/generate-ipc.mjs tests/portable --deny-warnings` | 0 / PASS | `changed-scope-lint.log`；不替代全仓 lint |
| `arkdock-native-preview --probe --sdk <SDK根>` | 0 / PASS（只读清单） | `environment.json`；不是原生通过开关 |
| `arkdock-native-preview --sdk <SDK根> --trust-fixture --output artifacts/native-preview/run-20261009-6` | 0 / PASS（独立后端 T1–T7） | `native-run-6.log` / `run-20261009-6/report.json` |
| `pnpm verify` | 1 / FAIL | `base-verify-final.log` / `artifacts/verify.json`；仅 Harness 与基线 UI smoke lint 两项 FAIL，其他 13 项 PASS |
| 已集成桌面 `pnpm verify:native` / `verify:release` | SKIPPED | 此隔离任务不连接或操纵另一会话应用；未打包、合并、推送或发布 |

该共同提交不含原工作树未提交的 AGENTS/CLAUDE/README 和大部分 docs。
聚合 Harness 检查因此失败；不覆盖或复制这些文件来掩盖集成前置。
基线 UI lint 错误在 `tools/ui-smoke.mjs`，本分支没有修改该文件。

## 调试过程中的真实失败

- PreviewBuild 未指定 `.preview` 输出根时 exit 1；源因及正确参数见协议/设置文档。
- 直接请求 `CompileArkTS` 未注册，exit 1；不作为适配器任务。
- run-1 exit 2，T1 查询通配符不受 tasklist 支持；改为精确进程名称。
- run-2/3 exit 1，构建图回执被 SDK 的 SGR 颜色转义遮蔽；缓存命中还需退出后
  重新读取日志。两项真实回归已加入，未把进程退出 0 当作画面成功。
- run-4 原驱动输出 PASS，但人工发现 clicked 仍 Count:0，整图变化只是按钮反馈；
  此 T3 证据作废。run-5 加入计数区域断言；最终 run-6 又加入源码标签区域断言，
  防止 Count 归零误证明源码文字更新。各旧报告保留，不以历史结果替代最终断言。

## 兼容与尚未通过的范围

独立 SDK 后端闭环 PASS，仅限此 Windows/SDK/Stage 样例与能力组合。
Tauri/WebView2 原始 ArrayBuffer 到实际前端 Canvas、DPI/letterbox 点击和前端
旧画面展示仍 BLOCKED（等待双方集成并运行桌面门禁）。Rust/Node 测试不代替它。
同进程状态保留热重载、滑动/滚轮、键盘/IME、旋转、其他 SDK、WSL 和远程
预览仍不准入。不宣称完整 ArkDock 前后端端到端验收已完成。
