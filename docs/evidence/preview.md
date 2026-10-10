# 调度与原生预览证据

2026-10-09，Windows 11 原生环境、HarmonyOS 26.0.0.105 / API 26 实测。
实际商业工具、官方来源、签名、摘要、产物及通信实验见 [SDK 证据](sdk-26-preview.md)。
完整原生驱动已通过；最终门禁与统计见 [命令记录](verification-2026-10-09.md)。

## 调度与监听

BuildCoordinator 发放结构化任务，epoch/session/build/generation 校验，单工程单任务，
变化防抖合并；构建中变化留下一代，旧完成不能释放新任务或覆盖新状态。
实际异步 SDK 构建退出 0 且收到有效首帧后才完成任务；接受当前原生帧才进入 Running。
编译失败保留旧画面、标旧并拒绝点击；修复实际 .ets 后重建恢复。
撤销信任/配置失效关闭执行 epoch、监听和原生 Job；重新信任建立新会话。

notify 只监听所选根，忽略 build、依赖、.git、.hvigor、cache/temp。
Windows OS watcher 集成回归实际写临时 .ets，核验生成文件被忽略及 Drop 通道关闭。
实际 native-counter 源码变更也经 watcher 触发新代次，未模拟文件事件。
不声明所有 junction 场景通过。

FrameMailbox 最新帧背压；session/build/generation 匹配，同一构建内 sequence 递增。
前端新构建序号可以从 1 重启，旧构建仍拒绝，实际回归已覆盖。
运行时失败撤销成功流，迟到帧不能恢复 Running；旧画面保留并标旧。

## 后端能力与资源

只读探测显式 SDK 目录与受限 metadata；未知版本/摘要、缺 SDK、非 Windows、
WSL/remote、未信任、旧会话均不能执行。配置匹配仅是前置，不能替代 D1/D2 实测。

当前适配器使用官方随包 Node/Hvigor assembleHap 与真实 modules.abc，
启动签名 Previewer。结构化参数、120s 构建超时、15s 握手/首帧超时，
挂入 kill-on-close Windows Job 后才恢复进程，停止只结束自有子树。
私有管道核验实际客户端 PID，回环图像端口核验自有 Previewer PID。
40 字节大端头/JPEG 限 1 MiB、360×640，完整图片解码后 PNG 重编码进入 IPC。
输入队列 16，点击核验当前 frame/build/generation、信任与边界；其他输入禁用。

完整源码刷新使用重建并重启，不声称保留组件状态热重载。
原生日志保留在 Temp/ArkDock-native 自有随机目录，每次上限 16 MiB，尚无自动留存 UI。
手机空轮廓仅是空状态；正式画面来自实际 SDK，不生成 HTML、固定截图或替身帧。

## 验收驱动

tools/native-smoke.mjs 使用用户授权创建的真实 examples/native-counter，
tests/fixtures/harmony-app 保持只读发现 fixture。
驱动检查 DevEco GUI 关闭、实际构建/帧、真实组件计数与帧变化、实际 .ets 改动后
新帧/日志、编译失败旧画面/输入拒绝与恢复、撤销信任中止/重新授权、新会话及 Job 停止。
驱动最终恢复源码、原先信任选择和自有 fixture；清理失败记 FAIL。
每个等待有期限，全局 15 分钟期限由驱动抛错并执行 finally；父门禁不强杀驱动。

verify:native 不读取 native-support.json，不接受历史 PASS，必须本次执行退出 0
且获得完整新记录。缺真实桌面/SDK/开发应用时 BLOCKED；实际断言或超时错误 FAIL。
verify:release 再次执行基础与真实 native gate，都 PASS 才打包，不自动发布。
