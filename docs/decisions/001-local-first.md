# ADR 001：本地能力与可核验失败

日期：2026-10-09；状态：采用。
默认 Windows 11 + Tauri 2/Rust + React/TypeScript/Vite，pnpm。
一个桌面工程，不建服务器/数据库/monorepo。受控会话登记作为可选增强。
协议单一 JSON 定义生成两端类型；无需额外代码生成服务。
未证实原生协议时实现探测、诊断与拒绝路径，不猜测 SDK 启动参数。
代价：本机无法验证原生闭环，发布门禁保持非成功；需 Windows/合法 SDK 接续。

pnpm 11 默认在 run 前自动安装。项目改为 verifyDepsBeforeRun: warn，保留
依赖状态警告但不让 doctor/便携检查触发下载/执行安装脚本；真正安装仍显式
执行且保留供应链政策。统一 verify 独立检查依赖，缺失继续返回 BLOCKED。

pnpm 11 内置 doctor 优先于项目同名脚本，不能用 package.json 覆盖。
项目工具链诊断使用 `pnpm run doctor`（或 node tools/doctor.mjs）。
保留用户需要的诊断内容；不改全局 pnpm、PATH 或 Shell 来伪装命令兼容。
