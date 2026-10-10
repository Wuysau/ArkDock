# 独立前端演示与浏览器验收

`pnpm dev:demo` 启动单独的 `http://127.0.0.1:1421/ui-demo.html`。
顶部永久显示 **UI 演示 · 非原生渲染**；场景仅由用户选择或验收脚本明确触发，
不会自动播放六阶段，也不会在正式应用缺 SDK 时接管原生桥接。

`pnpm build:demo` 只生成 `artifacts/ui-demo-dist`，不是发布包。
`pnpm build` 生成正式 `dist`；`pnpm test:ui` 检查生产包未包含 fixture、演示入口或测试图。

浏览器验收脚本默认先构建独立演示，再启动自己拥有的 Vite preview 1421 和私有无头 Chromium，
在结束后关闭。当前轮使用固定构建，不受并行编辑的 HMR 重载影响；manifest 保存构建文件哈希。
Windows 需安装 Chrome 或 Edge；自定义位置可设置 `ARKDOCK_UI_BROWSER`。
已启动独立演示服务时，可运行 `node tools/ui-visual.mjs --url=http://127.0.0.1:1421/ui-demo.html`。
脚本不控制用户的浏览器配置、目标应用或已有 ArkDock 进程。

结果保存到 `artifacts/ui-2026-10-09/checks.json`、`motion-samples.json`、`performance.json`、
`contrast.json` 和场景 PNG。
ArkDock 的实际前端表面覆盖 360×640、440×820、520×960，深浅外壳主题，
PNG 在上述尺寸之外另包含 44 DIP 的永久演示栏，manifest 明确记录这一区别。
另有 125%/150%/200% 浏览器 DPR。
DPR 是浏览器的逻辑尺寸验证，不能证明 Windows 真实多显示器 DPI、系统窗口吸附或焦点行为。
动态图证据是运行中的 CSS animation/transition 采样与快速切换/减少动画断言，不是 FPS 测量。
性能证据记录 200 个测试帧事件期间实际布局、样式、堆和标题区 DOM 变化计数。
对比度证据记录深浅外壳正文、说明 token 和实际主按钮的颜色与比例，不代表完整读屏审计。
`--quick` 仅供定位行为问题；会把 V1 截图标为 SKIPPED，不能代替最终完整验收。

`fixtures.ts` 通过显式 `UiBridge` prop 注入。测试图是带有“非 ArkUI · 非原生渲染”标记的
坐标校准网格；没有 React 记账业务，没有模拟 SDK、Shell 或窗口移动。
Windows 操作、目录对话框和 SDK 配置请求在演示中明确报不可用。
刷新、停止、候选与信任仅记录测试请求和测试状态，始终受演示标记约束。

本检查只能支持 V1–V4 前端证据。V5 必须使用真实 Windows 桌面与合法 SDK 的原生门禁，
不能引用这些 fixture 截图宣布通过。

共同布局增量使用 `node tools/ui-visual.mjs --joint-layout`，结果写入
`artifacts/joint-layout-2026-10-09`，保留原来的 UI 验收证据。
它额外覆盖未确认、共同最大化请求中、已确认、恢复中、受限五种显式测试场景，
以及窗口控制与 SDK/工程信任分离、请求去重、旧响应拒绝和窄窗菜单键盘操作。
所有布局事件都通过 fixture 驱动手动发出；此模式不会调整任何系统窗口。
`--joint-layout --quick` 用于行为定位，完整视觉验收仍需不带 `--quick` 运行。
