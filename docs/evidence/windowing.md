# Windows 外置窗口实现与证据

2026-10-09 在 Windows 11 家庭中文版 10.0.26300 x64 实际编译和运行，
MSVC 14.44.35207、Windows SDK 10.0.26100.0、WebView2 154.0.4258.62。
当前为单屏物理 2880×1800、192 DPI；已测单屏案例与未测双屏/权限案例分开记录。

## 窗口和资源边界

ArkDock 仍是独立顶层窗口；不注入、不跨进程 SetParent、不修改目标客户端。
EnumWindows 有上限，候选排除本进程、子窗口、owner 辅助窗口、tool/noactivate、
桌面/任务栏、隐藏/最小化/cloaked 窗口。DWM 可见边框和三点遮挡决定边缘候选；
释放时重新枚举并核验。遮挡检测对透明窗口采用保守矩形，可能有安全的假阴性。

新四边轮廓由 hook 线程拥有，使用 layered / transparent / noactivate / toolwindow
四个普通顶层窄窗口，不设全局 topmost，不成为目标子窗口。自有轮廓从候选遮挡中排除。
失败的部分显示、释放、无候选、解绑、退出都会隐藏/释放；窗口类注册成功后由 Windows
释放 class background brush，只有注册失败才自行 DeleteObject。

WinEvent 回调仅写有界事件队列，不查询窗口、定位、扫描工程或持业务锁。
安装线程建立消息队列并持续泵消息，完整处理销毁批次再处理命令；Drop 卸载自有 hook、
关闭只读 process handles、销毁轮廓并恢复线程 DPI 上下文。
目标生命周期核验包括 PID、精确创建 FILETIME、线程、HWND、销毁 epoch、存活状态。
事件连续性丢失或访问失败会解除，不结束目标进程。超快同进程 HWND 复用仍须压力实测。

跟随仅定位自有窗口，优先右侧、其次左侧，无空间则请求用户同意并排。
坐标为 physical screen pixels；DWM/outer 边框差值、工作区和负坐标保持独立。
SWP_NOACTIVATE/NOZORDER/NOOWNERZORDER/ASYNCWINDOWPOS 保留焦点与顺序。
目标最小化只隐藏自有窗口，恢复使用 SW_SHOWNOACTIVATE；用户最小化/最大化伴侣不被覆盖。
实际点击 ArkDock 后仍保持可见和自身焦点，恢复目标后不会抢回焦点。

## 明确同意和条件恢复

正常并排记录双方 original WINDOWPLACEMENT 和期望 outer rectangles。
最大化目标仅在同意后异步恢复正常；核验正常 placement 和伴侣未变，再计算并排。
请求接受不等于应用成功；750 ms 内观察实际期望矩形，才能提示已应用。
恢复直接使用原始 WINDOWPLACEMENT 的 workspace coordinates 和 show state，
不把 rcNormalPosition 当作 screen coordinates。原最大化状态也恢复。

解绑会再次 observe 当前矩形，保留已接受但尚未观察完成的请求。
750 ms 超时也转入最多三秒的取消追踪；只有原生命周期、原/期望矩形和未改变布局
匹配才补偿迟到请求。用户在任一窗口开始拖动/最小化、手动改变布局、身份失效或退出
会取消补偿。新拖动与应用退出不向目标排队旧位置恢复。
跨进程检查和 User32 异步写入不是原子事务；超过追踪期限或退出后的已接受请求无法撤回，
不承诺无限期冻结外部应用的消息队列。

## 本次证据

- cargo desktop check / clippy -D warnings 实际通过；日志见本次验证索引。
- geometry/lifecycle 风险测试在本机执行，涵盖溢出、负坐标、工作区、遮挡、弱生命周期及布局变更。
- 真实驱动：tools/windows-smoke.mjs + tests/native/window-host.ps1 /
  window-actions.ps1；用实际鼠标和两个同 PID 的独立 WinForms 窗口，读取真实 IPC。
- 四边轮廓 RED/GREEN、最大化原始拒绝、延迟队列立即解绑竞态都曾真实复现并保留日志。
- 驱动不提供 ArkUI 画面或工程构建；发现 fixture 只验证 B，不构成 D1/D2。
- 实际逐项状态、退出码与截图：[本次验证](verification-2026-10-09.md)；
  双屏不同 DPI/负坐标原生案例 BLOCKED，高权限/HWND 复用压力 SKIPPED。

## 官方依据

[WinEvent 消息循环](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook)、
[SetWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos)、
[ShowWindowAsync](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-showwindowasync)、
[GetWindowPlacement](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowplacement)、
[WINDOWPLACEMENT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-windowplacement)、
[SetWindowPlacement](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowplacement)、
[Extended styles](https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles)、
[窗口类画刷所有权](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-wndclassexw)。
官方 API 文档不是任意目标应用/Windows 版本的兼容认证。
