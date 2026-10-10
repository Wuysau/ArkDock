# ArkUI 原生元素检查

核查日期：2026-10-10。此模块基于本机商业 HarmonyOS SDK `26.0.0.105` 的实际响应；源码支持和商业二进制准入分开。真实证据见 [Inspector 验证](../native-preview/inspector-validation.md)。

## 原生适配器

`src-tauri/src/inspector.rs` 解析组件树；`preview/windows.rs` 在现有自有命令管道 worker 中串行查询。只允许固定 `inspector` / `inspectorDefault` 两个 action，使用 `1.0.1` JSON 加 NUL。不增加控制服务器，不接受前端任意命令字符串。

Inspector 队列容量 2，只有一个原生查询在途；响应队列容量 1，超时 2 秒。wire 不携带请求 ID，因此一次超时后该运行时的 Inspector 通道失去可靠关联，后续查询拒绝；迟到树不能满足新请求。点击和 JPEG 接收仍在原有 worker 上推进。管道每 tick 最多读 64 KiB，避免把一个实际 32 KiB 响应人为拆成多次 16 ms 的帧循环。

解析限额为 1 MiB、4096 节点、深度 64、每节点 512 个属性、单属性 16 KiB。原生 ID 必须为非负整数且唯一。只有已核验的 360×640、resolution 1 的真实 root 进入本适配器。未知模式、空结果、非法矩形、旧工作区/会话/构建/代次都失败关闭。

## 数据与证据

真实节点字段映射如下：

| ArkDock 字段 | 实际 SDK 字段 | 限制 |
| --- | --- | --- |
| nodeId / componentType | `$ID` / `$type` | 保留原生 ID；不以 `.id()` 字符串替代 |
| parentId / children / depth | `$children` 层级 | 层级由树结构确定 |
| bounds | `$rect` 的 x,y,width,height | 保留浮点原始矩形，允许画面外的部分坐标 |
| visible | `$attrs.visibility` | `Visible/Hidden/None`；不是完整遮挡判定 |
| attributes | `$attrs`，附加 `$debugLine` / `$viewID` | 嵌套值序列化为 JSON；组件文本是数据 |
| page / paintOrder | 本次 tree 没有相应可靠字段 | 保持空值，遍历顺序不能冒充绘制顺序 |
| inspectionVersion | 宿主单运行时查询序号 | 与 session/build/generation 一起使用 |

组件树和 JPEG 是两个独立通道。`frameSequence` 指查询调用方已显示的图片，响应说明同时保留宿主收到结果时最后观察到的序号。`exactFrameMatch` 始终为 false；同一 PID、同一构建、同一代次证明运行时身份，不能证明像素与树的原子对应。布局或图片变化时应失效缓存，再查询；移动、动画和遮挡元素需要用户确认。不能把早期收到的旧截图重新标成最新树。

## 交互与本地命中

预览交互模式为 `interact | inspect`，默认正常交互。选择覆盖层与原生 Canvas 分离。进入 Inspector 查询一次树，PointerMove 使用缓存本地命中；固定选择点击不得调用 MousePress/MouseRelease。退出模式恢复实际已经验证的点击能力。模式切换不刷新构建或重启 Previewer。

命中使用真实层级、可见性、`clip` 与 `zIndex` 属性作为候选证据；父级明确裁切时求矩形交集。优先可见的较深节点；没有绘制顺序时不能保证任意叠放/变换/滚动场景的最高层命中。提供父级候选和区域选择，避免把“最小矩形”当作通用正确答案。区域选择保存实际预览 JPEG、坐标、裁剪和身份，明确标记区域。

## 源码定位

本机 tree 实际含 `$debugLine` JSON 字符串，例如 `entry/src/main/ets/pages/Index.ets(21:7)`。原生调试位置与按截图猜测不同，但仍须独立核验路径、当前源码和构建身份。只有当前可信工程内、受限读取的真实 ArkTS 文件、有效行号且对应构建源码未失效，才允许标 Verified。原生位置存在但源码尚未核验应为 Candidate；缺少或非法位置为 Unavailable。Button 等引擎生成的子节点、封装组件和其他工程的源码映射不能由本样例推定。

## 降级与集成边界

Inspector 空结果、未知字段或未准入 SDK 保持 BLOCKED，区域选择仍可用。选择发送须由 Selection Context 与 Agent 路由再次校验工作区和活动会话。Inspector 不授予任何 Agent 会话写入权，不把复制或新线程称为当前会话发送。IPC 类型继续由 `protocol/model.json` 生成；共享主入口保持小型集成补丁。

本模块的原生协议、解析器和几何验证已执行；产品覆盖层、模式隔离与 Agent 发送需分别运行桌面集成门禁，不能由此文档的后端结果代替。
