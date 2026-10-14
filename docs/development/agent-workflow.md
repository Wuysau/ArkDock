# 多 Agent 开发与集成

长期工作目录仅为原始 ArkDock，检出 dev；main 仅保留稳定发布历史。并行任务允许创建临时 Worktree，验收完成后及时清理。流程为 **Worker Agent → Integration Agent → 用户验收**。

## Worker

1. 查询最新本地 dev、工作区状态及现有任务；从最新 dev 创建唯一任务分支和独立 Worktree，不与其他任务共享未提交文件。
2. 仅实现、验证和提交本任务；不直接修改 dev/main，不推送、发布或删除别人的工作区。
3. 修改 protocol/model.json 时同步生成 IPC 两端；跨模块接口按架构边界检查，工程关联与执行信任保持分离。
4. 交付分支、完整 Commit SHA、基线 dev SHA、修改范围、依赖、验证命令/退出码和未验收项；PASS、FAIL、BLOCKED、NOT_RUN 明确区分。

## Integration Agent

1. 读取 Worker 交付，检查实际差异和未提交内容；同时查历史与内容，避免重新引入 squash/cherry-pick 或已被其他实现覆盖的功能。
2. 对多个任务串行集成；从 dev 建立临时集成工作区，按依赖排序，完整有效分支优先 merge，部分有效分支选择性整合并记录来源。禁止用 ours/theirs 整体覆盖冲突。
3. 审阅并执行 pnpm verify（含 Rust fmt/clippy/test/build、TS、lint、Vite、IPC 和便携测试）；Windows desktop features 与相关进程、窗口、发现、信任和投递回归必须检查。需要原生运行验收时依公开验收指南执行；缺 SDK/协议/桌面时记录 BLOCKED，不能用基础测试代替。
4. 验证后重新检查 dev HEAD 和状态；纳入并发提交后重验，确认 dev 干净再 fast-forward。main 不随开发集成更新。
5. 向用户交付最终 SHA、构建产物和实际验证结果，待用户验收；无明确授权不推送远程、更新 main 或发布。

## 验收与清理

用户验收后，确认成果进入 dev、没有活跃任务或进程占用、未提交成果已保护，再清理任务 Worktree 和分支。历史未合并但内容已等效集成的分支，必须有核验记录和可恢复 Bundle 后才可删除。争议内容保留在本地归档，不盲目提交或丢弃。

任何破坏性清理前，仓库外保存 Bundle、stash 对象以及包含二进制的未提交/未跟踪文件，验证真实恢复与文件摘要。保留必要的本地 Harness、验收证据和忽略文件，禁止 reset --hard、强制删除未保存 Worktree 或强推。

## 本地生命周期工具

AGENTS.md 继续提供按任务路由的安全边界；它是本地入口，公共文档不携带机器路径、会话全文或密钥。Worker 可用 task:begin 登记，交付使用普通任务提交；Worker 不得调用 task:ready、task:finish 触发自动合入 dev。

现有 task-completion 的 ready/finish/Stop 自动快进工具仅供 Integration Agent 在明确授权且全部集成验证完成后使用；它不替代审阅、回归与用户验收。需要等待用户验收时不得用自动归档替代交付后的清理阶段。不需要将整套流程注入普通问答，也不建设新的任务调度平台。
