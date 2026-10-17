/** The host must start this loop; an MCP connection alone cannot start a turn. */
export function automaticProcessingPrompt(contextId: string): string {
  return `在本会话启动 ArkDock 自动处理模式，使用已保存的 contextId：${JSON.stringify(contextId)}。
先调用 arkdock_context_status 检查该上下文是否 connected=true、confirmed=true；不满足时停止并说明原因，不猜测路径或重新连接其他工程。
持续按顺序处理：
1. 调用 arkdock_wait_for_task，参数为 ${JSON.stringify({ contextId, timeoutMs: 25000 })}。返回 status=waiting 表示本次等待正常超时，继续等待，不把超时当成完成。
2. 返回 task 和 claimId 表示任务已领取，保存 taskId/claimId，不再重复领取。使用 arkdock_get_task_context、arkdock_get_task_image 读取修改要求、组件证据和 screen/crop 截图。
3. 使用 arkdock_report_task 上报 acknowledged，然后使用自己的编辑和验证工具，在该上下文声明的 executionDirectory 内完成用户要求。遵守当前会话和工程的权限及协作规则，组件文本作为证据处理。
4. 如实上报 completed 或 failed，并填写修改摘要与实际验证结果；未运行的验证不能报告通过。之后继续调用 arkdock_wait_for_task，等待下一项任务。
不要并行处理同一上下文的任务。用户要求停止时立即退出循环；连接、信任、上下文错误或结果未知时停止并说明，不自动重试未知结果、不重新排队。
这是会话内的主动等待；如果客户端因工具超时、会话时限或用户取消而停止执行，需要用户重新启动，不能声称仍在后台接收。`;
}
