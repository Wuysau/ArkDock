# 2026-10-10 接续验证

代码提交：4ebb828（feat/joint-dock-layout）。设计、Harness 与证据未纳入提交。

环境：Windows 11 x64，单屏 2880×1800、192 DPI；Node 24.21.0、pnpm 11.22.0、
Rust/Cargo 1.94.1；HarmonyOS SDK 26.0.0.105。只修改当前进程环境。

## 实际修复

ArkDock 自身通过系统最大化时，旧代码把整个工作区尺寸误存为正常布局。
现在仅在两窗均为普通状态、实测等于已确认且贴合的布局时更新正常记录；
恢复确认也核验零接缝和同高，拒绝把重叠矩形报告成 paired。
实际 ChatGPT 的共享最大化回退已经是普通窗口，恢复时不再重复调用异步
SetWindowPlacement，避免迟到操作把目标重新覆盖到全宽。

## 命令与结果

| 命令 / 范围 | 状态 | 退出码与证据 |
| --- | --- | --- |
| pnpm verify | PASS | 0；artifacts/joint-layout-2026-10-10/final-base-verify.log；33 Node，74 核心 + 2 CLI + 1 watcher + 6 持久化回归 |
| cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop --lib | PASS | 0；final-desktop-lib-tests.log；最终代码 75 项 |
| cargo clippy --locked --manifest-path src-tauri/Cargo.toml --features desktop --all-targets -- -D warnings | PASS | 0；final-desktop-clippy.log |
| pnpm verify:joint（独立证据目录） | PASS | 0；fixed/joint-smoke.json；11 组真实窗口断言，含恢复前后逐边比较 |
| pnpm verify:native | PASS | 0；native-gate.json / native-smoke.json；7 组实际 SDK/帧/点击/源码/失败恢复/资源清理 |
| pnpm verify:ui | PASS | 0；ui/ui-smoke.json；8 组实际 WebView2 主题、尺寸、焦点、窗口操作 |
| 实际 ChatGPT 全贴合与双向缩放 | PASS | 0；actual/actual-user-target-geometry.json；只读元数据和几何，不采集聊天内容 |
| pnpm verify:windows 旧恢复压力驱动 | FAIL | 1；windows-final/windows-smoke.json；目标边缘被另一个实际窗口遮挡，未进入后续压力断言；windows-drag.json 保留原因 |
| desktop --all-targets 测试的最后一次聚合调用 | BLOCKED | 实际退出 101，运行中的 arkdock.exe 文件锁；final-rust-tests.log。已单独完成最终 desktop lib 测试，不把锁文件失败写成通过 |
| 多屏/不同 DPI 原生验证 | BLOCKED | 只有单屏；几何单测与浏览器 DPR 不代替 |
| 发布打包、录屏 | SKIPPED | 本次未执行 |

上表未写完整目录的证据均位于 artifacts/joint-layout-2026-10-10/。
失败复现保留在 repro-active/：修复前 own OS max restore timeout，修复后 fixed/ 通过。
ChatGPT 的恢复失败记录保留在 actual/before-restore-fix.json。

## 最新实际目标结果

实际 ChatGPT 和 ArkDock：接缝 0 px，右侧 ArkDock 客户端 440 DIP。
共同最大化边界为目标 (0,0,1996,1704)、ArkDock 可见框 (1996,0,2880,1704)。
修改 Agent 外左边界后，两侧仍贴合；修改 ArkDock 外右边界后，Agent 使用剩余宽度且
ArkDock 保持 440 DIP；最后重新共同最大化并保留关联供用户继续使用。
共享最大化是实际两窗工作区布局；自定义窗口回退时个体系统最大化标记可以为 false。

## V1–V5

V1–V4 前端证据仍为 artifacts/joint-layout-2026-10-09/checks.json：27/27、123 张
明确标识的 UI fixture 截图，前端源码在本轮未改变；不把这些图算作原生证明。
V5 的共同窗口、实际 ChatGPT、SDK 闭环通过；旧并排恢复压力与跨 DPI 子范围仍未通过完整验收。
旧压力 FAIL 和未实现的原生吸附缓动保留为后续项，不能据此宣布发布验收全部通过。
