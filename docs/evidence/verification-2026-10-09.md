# 本次验证记录

日期：2026-10-09。环境与逐 ID 范围见 [兼容性矩阵](compatibility.md)。
以下结果来自本工作区实际执行。原生闭环仍为 BLOCKED；没有安装包、原生截图
或成功原生产物。Rust 风险测试已编写但未执行，不按测试替身计算产品完成比例。

## 实际命令

| 命令 | 退出码 | 状态与证据 |
| --- | --- | --- |
| `pnpm test:portable` | 0 | PASS：14 项，0 失败；契约、状态、门禁回归 |
| `node tools/check-format.mjs` | 0 | PASS：换行、空白、JSON 结构；不是 Rust formatter |
| `node tools/generate-ipc.mjs --check` | 0 | PASS：两端生成文件与同一模型一致 |
| `node tools/verify-harness.mjs` | 0 | PASS：入口体积、引用、命令与 CI 结构 |
| `node tools/check-boundaries.mjs` | 0 | PASS：UI 能力边界、capabilities 与正式路径扫描 |
| `pnpm verify` | 2 | BLOCKED：前述检查与 14 测试 PASS；前端依赖/Rust 缺失 |
| `pnpm verify:native` | 2 | BLOCKED：Windows 桌面、SDK、协议及 D1/D2 未验证 |
| `pnpm verify:release` | 2 | BLOCKED：基础依赖、Cargo.lock 与原生前置不满足；没有打包 |
| `pnpm run doctor` | 2 | BLOCKED：Node PASS；WSL、Cargo 缺失、SDK 未配置 |
| `pnpm dev` | 2 | BLOCKED：非 Windows，未启动桌面进程 |
| `pnpm desktop:build` | 2 | BLOCKED：非 Windows，未生成安装包 |
| `pnpm install --lockfile-only --offline --ignore-scripts --frozen-lockfile` | 1 | BLOCKED：离线 metadata-full 缺失，未完成供应链校验/安装 |

基础报告：`artifacts/verify.json`（2026-10-09T04:44:59.525Z）。
发布报告：`artifacts/verify-release.json`（2026-10-09T04:46:05.171Z）。
原生报告：`artifacts/native-gate.json`（2026-10-09T04:46:05.164Z）。
这些是本地生成且忽略提交的报告；重新执行对应 gate 会更新，文档保留本次结果。
退出 2 的 BLOCKED 约定仅适用于 ArkDock gate；普通编译器非零仍为 FAIL。

14 项便携测试覆盖生成契约与命令参数、原生不可用拒绝、诊断不执行 SDK 文件、
门禁退出分类、请求范围、快照/帧版本拒绝、250 候选搜索不截断及无可用候选兜底。
范围：[便携测试目录](../../tests/portable)。它们没有执行 Rust resolver、窗口 API
或 Previewer；不能证明对应原生验收通过。

## 额外检查与限制

- 已用现有 Python YAML 解析器检查锁文件与 CI YAML 语法，核对 145 个选定依赖
  节点的完整闭包与精确根依赖；不能替代 pnpm 安装与版本兼容验证。
- 使用本机现有 TypeScript 5.9.3 解析器只读检查 TS/TSX，语法诊断为 0。
  项目锁定 TypeScript 6.0.3 尚未安装；这不是 typecheck 或 React 构建 PASS。
- `cargo`/`rustc` 不在 PATH；直接探测退出 127，门禁 spawn 报 ENOENT。
  Rust fmt/clippy/test/build 和 Windows desktop feature 均未验证；未生成 Cargo.lock。
- pnpm 11 的 `pnpm doctor` 是内置命令，实测运行的是 pnpm 自身诊断并因网络
  退出 1。项目文档改为 `pnpm run doctor`，未修改全局 pnpm 配置。
- 初次 `pnpm verify` 遇到 pnpm 11 自动安装尝试，网络 EPERM 后终止本次自有
  命令，退出 130。项目 `verifyDepsBeforeRun: warn` 避免只读诊断隐式安装；
  完整 gate 仍阻塞缺失依赖，供应链策略没有关闭。
- CI 已配置，未远端运行；没有真实桌面、双屏或 SDK 兼容性证据。

## 独立审查后的修正

独立源码审查提出的范围问题已修正：
- 修改请求携带目标/session 范围，弹窗前后核验，拒绝旧请求操作新目标。
- 模块配置/执行元数据纳入信任指纹；回归已编写，Cargo 缺失待运行。
- 手选目录与 inspector 同时保留 WSL UNC 身份；回归已编写待运行。
- 只有原生自有 gate 的退出 2 是 BLOCKED；编译器非零为 FAIL，便携回归通过。
- 无效/不支持环境候选保留说明；无可选本地候选时显示目录兜底，回归通过。
- Window/文件事件用合并邮箱；停止/切换替换旧监听邮箱，避免队列满丢变化。
- 同步后端返回前合并窗口/文件变化；只有当前原生帧可进入 Running，旧结果拒绝。
- Linux 模块访问器由真正几何/恢复断言覆盖，未用 lint 抑制。

本机无法 Cargo 编译，以上静态审查不构成 Rust 或 Windows PASS。
