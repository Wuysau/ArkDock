# 兼容性与验收矩阵

2026-10-10 集成交付：基础检查和最终 desktop,native-tools 125 项 PASS；独立 SDK T1–T7 PASS。
0.1.0 未签名 x64 NSIS/MSI 构建 PASS；NSIS 安装/生产 UI 与 Rust IPC/无 SDK/关闭/重启/卸载 PASS。
安装包排除两个源码开发 CLI 和 SDK。MSI ALLUSERS=1，未做系统安装测试。
本次 UI 7 组通过但角落缩放 FAIL，joint/native gate 在真实绑定前置 FAIL；用户暂不继续鼠标验收。
本次结果优先看 [集成交付记录](integration-delivery-2026-10-10.md)，下方历史 PASS 不能覆盖这些失败。

2026-10-10 增量结果：[真实共同布局与前端接续验证](joint-layout-2026-10-10.md)。
共同窗口 11 组、实际 ChatGPT 零接缝/双向缩放/共同最大化、当前 SDK 7 组、真实 UI 8 组 PASS。
旧恢复压力门禁本轮未完成，跨 DPI 仍 BLOCKED；下方历史子范围不等同本次全部通过。

2026-10-09 当前为 Windows 11 原生工作区，非 WSL。
Windows 11 家庭中文版 10.0.26300 x64；PowerShell 7.6.6；Node 24.21.0；
pnpm 11.22.0；Rust/Cargo 1.94.1 x86_64-pc-windows-msvc；
MSVC 14.44.35207，Windows SDK 10.0.26100.0，WebView2 154.0.4258.62。
仅当前进程 PATH 使用 portable Node/pnpm；没有改变全局配置。
本机单屏 2880×1800 / 192 DPI。Command Line Tools 26.0.0.851、
HarmonyOS SDK 26.0.0.105 / API 26、Hvigor 6.26.8、ohpm 26.0.0.630。
不将本次单机组合扩展为所有 Windows 11 版本/外部应用的兼容认证。

| 范围 | 状态 | 实测/限制 |
| --- | --- | --- |
| pnpm 冻结安装及供应链校验 | PASS | 锁定安装完成，含官方项目内 Tauri CLI 2.12.1 |
| React 18.3.1 / TS 6.0.3 / Vite 6.4.3 / Oxlint 1.76.0 | PASS | 实际 lint/typecheck/生产构建 |
| Rust 核心与 desktop feature | PASS | Cargo.lock、fmt、test、build、check、clippy -D warnings |
| Windows 外置单屏联动 | PASS / FAIL（子范围） | 基础拖动/跟随/最小化通过；本次最大化并排/延迟恢复压力回归失败，不能维持全范围 PASS |
| Windows 双屏跨 DPI/负坐标 | BLOCKED | 本机仅单屏；几何单测 PASS 不算双屏实测 |
| 权限差异/HWND 复用压力 | SKIPPED | 未执行原生压力/高权限案例 |
| WSL/remote 原生运行 | 不支持 | 身份保留，拒绝隐式 Windows 路径转换 |
| 浅/深/系统主题及自适应 UI | PASS | 实际 WebView2 五组断言和四张界面截图 |
| HarmonyOS 商业 SDK | PASS（当前配置） | 用户安装的签名 Previewer/manifest/摘要与实际构建、协议；其他版本未核验 |
| OpenHarmony 独立工具包 | SKIPPED | 仅公开源实现参考，未使用它替代商业 SDK 验收 |
| 实际原生画面/点击/源码刷新 | PASS（当前配置） | 真实计数器、完整重建、失败恢复与自有进程停止 |
| 安装包与发布 | 待最终记录 | release 正在执行，实际产物结果见验证记录；不发布 |

## A/B/C/D/E 验收状态

复合 ID 的子范围分别记录，不能以单机/单测 PASS 覆盖剩余原生项目。

| ID | 状态 | 证据与剩余范围 |
| --- | --- | --- |
| A1 | PASS（本机案例） | 真实拖动、释放、四边高亮、移动缩放、同 PID 两窗口选择 |
| A2 | PASS / SKIPPED | 实际最小化恢复/关闭/会话清空 PASS；高权限/HWND 复用压力 SKIPPED |
| A3 | PASS / BLOCKED | 本机 192 DPI 点击不隐藏/抢焦点 PASS；双屏负坐标 BLOCKED |
| A4 | FAIL（本次压力回归） | 基础同意/正常恢复通过；最大化迟到补偿与恢复前几何核验出现失败，保留完整断言与保护条件，待定位 |
| B1 | PASS / BLOCKED | Rust 弱/强决策规则 PASS、实际 UIA 唯一弱候选仍待确认 PASS；默认强自动来源未完成 |
| B2 | PASS / BLOCKED | TTL/错配/伪造拒绝与选中标签 metadata PASS；认证登记/宿主活动工程证明 BLOCKED |
| B3 | PASS（测试范围） | JSON5 只读、路径/worktree/环境、受限 monorepo、配置/分配预算回归；不声明所有外部工具探测 |
| B4 | PASS（测试范围） | 信任独立、撤销、根/模块配置改变撤信任、历史不保存运行期句柄 |
| C1 | PASS（测试范围） | 合并/串行/代次、OS watcher、实际 ArkTS 刷新/失败恢复、构建中撤销与重新授权 |
| C2 | PASS（测试范围） | 旧任务/帧/输入拒绝、背压、真实失败保留旧画面、停止清理自有 Previewer Job |
| D1 | PASS（当前 SDK） | DevEco GUI 关闭，实际计数器构建/HAP/Previewer/帧，当前 26.0.0.105 配置 |
| D2 | PASS（当前能力） | 实际鼠标点击 Count 0→1、真实 .ets FIRST→SECOND、带构建/代次日志与帧证据 |
| D3 | PASS | 编译后的明确拒绝与便携回归、实际 native gate 退出 2，无正式 mock 兜底 |
| E1 | PASS（结构范围） | Harness/生成 IPC/能力边界检查，不能证明所有 Agent 客户端加载 |
| E2 | PASS / SKIPPED | 基础/doctor/native PASS；release 最终结果见验证记录；远端 CI SKIPPED |
| E3 | PASS / SKIPPED | 文档接续与独立源码审查更新；新 Agent 人工接续验收 SKIPPED |

## 不可越界的结论

当前 UIA verifiable 表示采集了所绑定窗口的 selected metadata，activeScope=false，
不表示已经认证了宿主的工程语义。文件登记、标题、历史和最近修改时间不能自动关联。
SDK manifest 版本也不是工具真实性或兼容证明。
当前结论包含实际签名、摘要和完整原生驱动，仍只适用于此次商业工具包；
Stage 默认目标、pages/Index、360×640。滑动/文字/旋转/保留状态热重载不支持。
tests/fixtures/harmony-app 只用于发现和安全测试，不能用于 D 验收。
设备轮廓没有假 ArkUI 页面，正式预览只接收实际鸿蒙工程画面。

详细证据：[窗口](windowing.md)、[发现](discovery.md)、[调度/预览](preview.md)、
[真实命令与日志](verification-2026-10-09.md)、[操作手册](../runbooks/windows-native-preview.md)。
上次受限 WSL 无 Cargo/网络/互操作的记录是历史，本次相关阻塞已消除。
