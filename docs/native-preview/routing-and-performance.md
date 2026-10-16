# 自定义路由与交互性能

## 路由适配

普通 Previewer 无法自动获得应用的系统路由构建器时，ArkDock 在自有工程快照中读取模块 router map，使用已安装 SDK 的 ArkTS parser 生成静态 navDestination 适配。保留项目已有自定义 dispatcher，给同名 builder 分配独立导入别名，检查跨模块依赖及环，只在快照中补齐需要的模块依赖。

快照内依赖链接重新定位到真实内容；不修改原工程源码，不把设备签名凭据复制到预览快照。构建、页面资源、ABC 与 Previewer 启动都使用同一个 BuildArtifactContext。初始页面从真实编译资源读取，不固定假设 pages/Index。

路由与快照限定为所选 HAP 的可达依赖，不串联其他独立 HAP。原生运行结束后后台回收本次自有快照及日志，遇到占用有界重试，不遍历目录联接的目标；诊断日志需要在结束当前运行前保存。当前仍复制完整的可达工程及 wrapper 缓存，尚无内容寻址增量快照；输入与渲染热路径不执行这些复制。

检查器仅将有证据的结构化 $debugLine 还原到原始文件，校验原文件及适配文件摘要并还原 UTF-16 插入偏移；注入的代码没有原始位置，不能伪造定位。映射仍只是源码候选，后续选择核验继续执行。页面内容、普通字符串和绝对路径不会被全局替换。

真实 jingmo 工程已在隔离目录完成 PreviewBuild，适配 100 个静态路由；原工程路由源码保持不变。这证明编译链路，不替代用户点击“经典诗文”及其他路由的画面验收。运行时计算生成且不在静态配置中的路由，须由项目已有 dispatcher 处理；不猜测动态模块代码。

参考 [Navigation 跨包路由](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/ui/arkts-navigation-cross-package.md)、[JSNavigationStack](https://github.com/openharmony/arkui_ace_engine/blob/master/frameworks/bridge/declarative_frontend/jsview/js_navigation_stack.cpp) 与 [Previewer 启动参数](https://github.com/openharmony/ide_previewer/blob/master/util/CommandParser.cpp)。官方 Debug Preview 的 srmPath 属于另一套启动生命周期，不能直接向当前普通 Previewer 填参数并宣称支持。

## 交互优化

- 原生控制回执主动推送，前端按操作身份等候；有界补偿查询处理订阅竞态，不再为每个输入固定等待轮询间隔。
- 新帧到达唤醒唯一二进制读取流水线，保留有界恢复轮询；后台或隐藏后暂停渲染，无每帧 React 全树刷新。
- 原生循环每轮在时间与数量预算内批量读取消息；完整 JPEG 解码校验移到独立线程，最多一个处理中、一个待处理和一个结果，不积压旧帧。
- 控制回执、表面尺寸和路由上下文随收到帧时绑定；校验延迟不能把命令之前的帧误记为命令之后的确认。
- SDK 哈希探测结果供能力显示复用，高频输入使用已核验运行代次；源码通知立即失效，定期审计保底，构建/部署前仍重新验证信任与实际输入文件。
- 正常输入默认可用，可显式暂停；暂停同时阻断旧点击回退。切工程、重建、失焦、隐藏与 Inspector 模式仍取消进行中的手势。

能力面板按秒采样显示实际绘制 FPS、二进制读取和输入回执延迟的 p50/p95；每种延迟最多保留 120 个样本。静止画面 FPS 低不自动表示卡顿，命令 ACK 也不等于屏幕像素响应。

与 DevEco Studio 对照时使用同工程、同设备尺寸与 SDK：连续滚动 60 秒、输入相同文本、重复页面切换，分别记录 FPS、ACK/IPC p95、CPU/内存及可见延迟。当前没有这组真人操作测量，不能宣称已经达到 DevEco 性能。设备画面采用独立 PNG 截图刷新，详见 [设备运行](../native-controls/device-runtime.md)。
