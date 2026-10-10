# 原生预览性能证据

## 基线：2026-10-10

基线先于 Frame Renderer 实施采集。Windows 真实 WebView2，合法本机 SDK
26.0.0.105，实际 `examples/native-counter` ArkTS 工程；不是浏览器替身。
运行的是本任务创建的独立 ArkDock PID 44652，以及该进程拥有的 Previewer。
原生 exe 从已构建 dev 基线复制，前端从独立 worktree 提供；该 exe 并未在本次
基线采集前重新编译。因此只比较实际加载的传输路径，不将它当作新分支发布验收。

```powershell
$env:ARKDOCK_SDK='D:\CodingTools\HarmonyOS\command-line-tools'
$env:ARKDOCK_PROBE_PID='44652'
node tools/performance-probe.mjs baseline
```

退出码 **0 / PASS**。证据为 `artifacts/preview-performance-baseline.json`、
`artifacts/preview-performance-baseline.png`。连续 12 次点击均以实际 ArkUI
计数日志确认；会话、构建 ID、generation 保持不变，设备边框重挂载 0 次。

| 指标 | 样本 | p50 ms | p95 ms |
| --- | ---: | ---: | ---: |
| 点击捕获 → click_preview 发出 | 12 | 0.20 | 0.40 |
| click_preview 往返 | 12 | 11.90 | 25.60 |
| 点击 → 浏览器有效画面解码后 rAF | 12 | 180.70 | 305.20 |
| PNG 事件到达 → 解码后 rAF | 12 | 3.70 | 7.40 |
| img.decode 等待 | 12 | 0.80 | 1.60 |
| Rust capturedAt → PNG 事件到达 | 34 | 124 | 154 |

点击时序采样在 DOM click 的捕获阶段；发出时间使用仅作用于此 WebView 的
Tauri 桥接包装器，完成后恢复。展示时间由真实 image.decode 完成后下一次 rAF
确定，不声称为显示器实际发光时间。匹配帧满足 capturedAt ≥ 点击时间；引擎
持续渲染时该指标是点击后首张可呈现帧的时间，不能单凭该指标证明像素内容已
包含点击结果，另以真实 ArkUI 计数日志确认输入确已执行。

这一采样期 WebView `TaskDuration` 增量为 0.232545 秒，JSHeapUsedSize
11,531,488 → 17,671,560 字节。该堆包含采样器保留的事件记录，不能据此判断
产品内存泄漏。Previewer 内部 render 时间、原生 CPU 百分比未暴露，未测量。

## 已核对的路径与根因边界

- `parse_jpeg40` 对 JPEG 做一次完整尺寸/分配限制解码；兼容 `legacy_png`
  再解码同一 JPEG、PNG 编码、base64 编码，再由 WebView 解码 PNG。
  未启用已有二进制入口时，这些转换在每帧路径上实际存在。
- 桌面轮询接收与上述兼容转换位于原生 worker；34 个基线帧的 Rust 收到到
  PNG 事件到达延迟显著高于浏览器 decode/rAF 等待。尚未将 worker 调度和
  每个编码阶段独立计时，因此不能把全部 124/154 ms 归因于 PNG 编码。
- App 普通点击使用全局 pending，使输入表面在请求期间暂时禁用；这能解释
  连续输入的门禁抖动，不能解释所有上游帧延迟。
- 原有 `first-frame` class 虽一直存在，但同 session 的设备 shell 没有重挂载；
  这次真实基线未发现普通点击重播该动画。不能将 CSS 动画列为已复现根因。
- 所有 12 次普通点击未改 build/session/generation，没有发现误触重建或重启。

## 实施与回归边界

主路径复用 `enable_binary_preview_frames` → `read_preview_frame` →
`disable_binary_preview_frames`。只有成功启用回执之后拉取，当前消费者 ID
贯穿三个命令；旧消费者清理不得取消同 session 的后继消费者。
单路读取上限 30 Hz，空邮箱指数退避到 250 ms，页面隐藏时暂停。
仅明确版本/命令不支持后接收兼容 PNG，不因一般错误启动竞争消费者。

封包在浏览器解码前核验小端 metadata 长度、UTF-8、协议、工作区/会话/
构建/generation/sequence、维度/像素/压缩载荷上限以及 JPEG SOF 内尺寸。
`FramePresenter` 最多一张正在解码、一张待处理和一张已显示 bitmap；解码后
再次检查当前身份与最新帧，过期 bitmap 立即 close。绘制前保留上一张，缩放/
DPI 变化同步重绘最后有效 bitmap，不因新帧先清空。

只有 `markDisplayed` 后的帧才供点击与 Inspector 使用；最新收到但尚未解码的
帧不会冒充已显示画面。构建失败保留旧图并沿用明确旧画面门禁，真正停止、
失效或更换生命周期时清空并回收。

`node --test --test-isolation=none tests/portable/frame-renderer.test.mjs
tests/portable/ui-state.test.mjs tests/portable/view-state.test.mjs` 退出 **0**，
25/25 **PASS**；包括协商时序、旧 lease、包大小、异步过期、资源释放和
displayed/received 分离。这是契约/状态回归，不能替代 Windows 原生验收。

## 优化后：2026-10-10

以本分支重新编译的 debug exe、独立 PID 54208、相同本机 SDK 和同一实际
计数器工程采集。命令 `node tools/performance-probe.mjs optimized`，传入
`ARKDOCK_PROBE_PID=54208` 和上述 SDK 环境；退出 **0 / PASS**。证据为
`artifacts/preview-performance-optimized.json`、`artifacts/preview-performance-optimized.png`。
这次同时包含前端和二进制消费者租约等原生集成变化，不能当作只改变单个变量
的微基准。该最终 exe 包含截图留存、静态帧和原生可见性补丁。

| 指标 | 基线 p50 / p95 ms | 优化后 p50 / p95 ms |
| --- | ---: | ---: |
| 点击 → 浏览器有效画面解码后 rAF，12 次 | 180.70 / 305.20 | 140.00 / 207.30 |
| 点击 → click_preview 发出，12 次 | 0.20 / 0.40 | 0.20 / 0.30 |
| click_preview 往返，12 次 | 11.90 / 25.60 | 14.00 / 17.30 |
| 浏览器收到压缩帧 → 解码后 rAF，12 次 | 3.70 / 7.40 | 6.40 / 10.60 |
| 浏览器解码等待，12 次 | 0.80 / 1.60 | 1.70 / 2.80 |

点击后反馈 p50 降低约 **22.5%**、p95 降低约 **32.1%**。JPEG/ImageBitmap 的
浏览器解码等待并未比 PNG/img.decode 更快；观测到的收益来自整体接收、转换
与传输链路变化，不能声称由 React 或浏览器解码单独带来。

优化后采样末尾 Canvas presenter 累计展示 52 帧、解码过期丢弃 0；探针记录
51 个展示事件（安装探针前首帧已绘制），63 次串行读取。该数不包含原生最新
帧邮箱覆盖计数，也不声称无任何生产者帧丢弃。本场景原 PNG/base64 的事件
载荷长度中位数约 74,552 个 ASCII 字符，JPEG 中位数 19,095 字节；仅对这个
画面样本成立，不推广为固定压缩率。

优化后 WebView TaskDuration 增量 0.257937 秒，JSHeapUsedSize
6,641,716 → 6,230,616 字节。采样堆/GC 和之前页面生命周期不同，不能把这些
单次数字解释为长期 CPU 或内存保证。原生总 CPU、引擎内部 render、显示器
实际发光延迟依然没有测量。

实测还暴露并修复了开发时钟错误：把 `clearTimeout` 直接保存为 clock.cancel
并以 clock 作为 this 调用，在真实 WebView2 抛出 `TypeError: Illegal invocation`；
初版 wake 因此阻断点击。现已用箭头函数调用全局 timer，并将输入锁的释放置于
try/finally。新增契约回归验证时钟 receiver；以上 PASS 数据来自修复后真实运行。
每次 lease 协商另外使用唯一递增后缀，覆盖同一 session 暂时不可用后恢复时
迟到清理的竞态；旧 disable 不能取消其后继 owner。
较早集成阶段 PID 41804 的 12 次采样为 112.60 / 206.50 ms，旧回执保存在
`.local/preview-performance-optimized-initial-source.json`；它与最终 UI 布局/原生
版本不同。主报告使用上述最终 140.00 / 207.30 ms，不能挑选最低单次结果。

## 最终原生门禁与压力实验

`ARKDOCK_TEST_PID=54208`、`ARKDOCK_TEST_PORT=9447`、上述 SDK 环境下运行
`pnpm verify:native`，退出 **0 / PASS**。该 PID 来自本任务复制的独立
`arkdock-inspector-native.exe`，不枚举第一个同名用户进程。新证据为
`artifacts/native-gate.json` 与 `artifacts/native-smoke.json`，覆盖 8 组真实断言：
SDK 构建/Canvas、撤销/重新信任、ArkUI 点击、消费者权限拒绝且无兼容事件、
真实 `.ets` 更新重建、编译失败旧画面像素保持与恢复、停止及仅终止自有 Job。

已保留先前 FAIL：一次 SDK 取消构建留下自有 fixture 的零字节
`.hvigor/cache/meta.json`；一次前序解除关联的异步位置恢复与新拖动相撞。
仅移走该明确自有的零字节生成文件，未清理用户 SDK 或其他工程；新驱动按原生
runbook 等待 3.5 秒并核验窗口位置稳定后执行相同真实拖动。失败回执保留于
`.local/`，上述新 PASS 没有删除任何门禁断言。最终窗口可见性版本曾在拖动
bootstrap 再次遇到空闲状态超时；对已解除关联的专用应用执行 Page.reload 后，
重新运行相同门禁 8 组断言全部 PASS。该重试不恢复或伪造目标绑定。

`node tools/performance-probe.mjs stress` 临时为实际 ArkTS 计数器加上 50 ms
`@State` 更新，在合法 SDK 中实际重新编译和运行；finally 停止、恢复源文件与
原先信任，并解除自有 fixture。最终 PID 54208 的实验记录 **155** 个实际呈现样本 /
**11,508 ms**，序号严格递增、同一构建、Canvas 对象保持、解码过期丢弃
1。192 DPI 下实际窗口 clientHeight 为 1280 / 1425 / 1585 px，Canvas 高度为
248.5 / 321.5 / 401.5 CSS px，backing 高度 497 / 643 / 803 px。初次使用的高度
低于应用 640 DIP 最小高度，因此不能当作实际缩放测试；修正为 640/720/800 DIP
并按真实 DPI 转换后才得到这些变化。

最小化段另发现实际 WebView2 行为：scoped `minimize_window` 后，Win32
IsIconic 连续为 true，而 `document.hidden` 连续为 false，期间仍呈现 17 帧。
仅依赖 DOM visibilitychange 无法证明暂停。现已接入原生窗口可见性通知，Canvas
以独立 visible 门禁暂停并保留 bitmap。最终 `preview-performance-stress.json`
记录原生 IsIconic 连续为 true、DOM hidden 仍为 false，暂停段实际呈现帧 **0**，
恢复后同一 Canvas 恢复更新；退出 **0 / PASS**。前序 FAIL 回执保留于
`.local/preview-stress-before-native-visibility-fix.json`。

双屏/跨屏 DPI、尚未合并的悬浮球分支共同回归、Previewer 内部 render/原生 CPU
测量仍需单独真实证据；不能由便携单测或这个计数器采样代替。
