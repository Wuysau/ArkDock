# 模拟器与真机运行

设备运行独立于 Previewer。它使用可信工程的 assembleHap、SDK 签名验证工具、HDC 与设备上的 aa/bm/UItest；独立的 run/target/generation 身份和 ADDF 二进制 PNG 帧不能由 Previewer 的画面或构建收据代替。

## 已实现链路

1. 显式连接后发现真实 HDC 目标。stdout/stderr 分离、有大小和超时限制；仅根据真实列表选择目标。用户选择模拟器或真机，不根据 USB/TCP 名称猜测类型。
2. 在后台构建当前工程 HAP，核验构建任务收据、实际输出、文件摘要、应用/模块/Ability、签名和 Profile。构建前后及安装前检查源码指纹；产物复制到自有目录。已有签名配置由工程构建工具使用，不自动创建或导出密钥。
3. 安装须确认准确目标和本次产物。短时单次 token 绑定当前运行身份、目标及摘要；重新选择或断开使旧授权失效，源文件变化由安装前的完整指纹校验拒绝。调试 Profile 的设备清单按 UDID 或 SN 精确比对。
4. 安装和启动均解析实际回执，安装后查询包元数据，采集和输入时再核验进程及前台窗口。退出码 0 或文件存在不能单独表示成功。
5. 单次截图和连续刷新拉取真实设备 PNG，校验格式、解码尺寸和预算后绘制。输入要求截图授权、输入授权、当前目标、前台应用证据及 5 秒内的当前帧；支持点击、双击、长按、滑动、拖动、Unicode 文字与限定按键。
6. SDK Emulator CLI 读取本机实例；可启动指定实例并连接其明确 HDC 端点。停止仅作用于 ArkDock 自己启动并持有的进程；不结束用户已有模拟器，不自动下载镜像、接受许可或重置数据。

所有长操作在独立 worker 上串行执行，Windows 子进程在恢复主线程前加入自有 Job；取消、超时、输出超限只结束自有进程树。HDC 复用兼容的已有 server，使用禁止自动拉起的客户端模式；不存在 server 时才持有自己的前台 server。HDC 有全局单实例约束，不宣称能在另一个端口任意并存服务，也不执行全局 kill。

## 图像与性能范围

当前设备画面是有界截图刷新：每次完成后间隔至少 350 ms，还受 screenCap、传输及解码耗时影响。它适合运行验证与操作，**不是 60 fps 投屏协议**。前端最多一个读取/解码流水线，切目标、取消后拒绝迟到画面；修改源码不影响设备已经安装的版本，更新版本必须重新构建部署。Previewer 的事件驱动画面优化与设备截图刷新分别统计，不能混用帧率。

## 环境要求

- 已安装且受支持的本机 SDK、Node/Hvigor、Java、HDC；需要模拟器时还须合法可用的镜像与实例。
- 工程依赖已安装，HAP 具有合法有效的签名配置；调试设备在 Profile 许可清单中。
- 设备已连接且授权调试；设备具备 aa/bm/UItest 与可核验的前台窗口信息。无法证明前台或签名时给出具体错误，不能通过去掉校验继续输入/安装。
- 完整原生输入法 preedit、应用专用高帧率录屏服务和多显示器设备输入未实现；本机输入法提交最终文字可用。

## 用户验收步骤

自动化检查只覆盖解析、身份隔离、有界进程/图像、命令参数及源码/产物校验；真实模拟器与真机全链路仍须用户分别验收。

1. 关联可信工程，进入设备运行，允许连接，发现并选择准确目标。无 Previewer 构建也应可操作。
2. 构建 HAP；确认显示的应用、目标与摘要后安装，再启动。记录构建、签名、安装、启动各自结果。
3. 授权截图并查看真实设备画面，再授权输入，逐项测试点击/长按/拖动、中文及按键。切到其他前台应用后输入应停止。
4. 截图期间切目标或取消；旧画面与旧输入应失效。修改源码后尝试部署旧准备产物应被拒绝；已经安装的版本可继续运行。断开不能结束已有共享 HDC 或用户模拟器。
5. 模拟器选择已有实例，启动、连接、执行上述操作，再停止 ArkDock 自己启动的实例。目标断开、签名过期/不匹配、缺 Java/UItest/镜像时应给出具体原因。

## 官方参考

实现参考公开协议和本机工具帮助，自行编写 Rust/TypeScript 适配；不分发 SDK、设备镜像、私钥或官方二进制。

- [HDC 文档](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/dfx/hdc.md) 与 [HDC server](https://github.com/openharmony/developtools_hdc/blob/master/src/host/server.cpp)：服务生命周期、目标及文件传输。
- [HAP signer](https://github.com/openharmony/developtools_hapsigner/blob/master/README_ZH.md)：verify-app、verify-profile 与实际签名验证输出。
- [bm 工具](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/tools/bm-tool.md)、[aa 工具](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/tools/aa-tool.md)：安装信息和 Ability 生命周期。
- [UItest](https://github.com/openharmony/testfwk_arkxtest/blob/master/uitest/server/server_main.cpp)：截图与输入回执；HDC 重建远程命令后仍需 POSIX 引号保护。
- [设备 ID](https://github.com/openharmony/startup_init/blob/master/services/modules/udid/udid_comm.c)：UDID/SN 精确核验。
- [官方 Emulator CLI](https://developer.huawei.com/consumer/cn/doc/doccenter-deveco-studio/ide-commandline-emulator)：本机实例生命周期。
