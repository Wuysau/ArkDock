# HDC 设备运行接入边界

本次交付独立 `DeviceRuntimeAdapter`、受限 `HdcOperation` 命令计划及真实 stdout 的严格设备列表解析器。它没有执行 HDC，也没有完成安装、启动、截图、输入或模拟器运行。全部运行、构建、测试和验收状态为 **NOT_RUN — 等待用户验收**。

设备运行与 Native Previewer 分属不同后端。Previewer 的命名管道、JPEG 帧头、Inspector 构建关联及能力回执不能作为设备画面的证明。

## 已接入代码

- `src-tauri/src/preview/device_runtime.rs`：在显式选择的 SDK 下只读检查固定 HDC 路径、规则文件及 SHA256；提供 state/action/cancel 接口。state 不启动设备枚举。workspace/session/build/generation/尺寸身份变化立即释放旧选择。
- `src-tauri/src/preview/device_runtime/plans.rs`：Rust 构造限定操作参数，不开放任意 Shell；每个设备操作都必须指定 `-t`。列表最多 64 个目标，stdout 最大 64 KiB，单行最大 1 KiB，参数总量最大 4 KiB，计划中的客户端超时为 5 秒。
- Discover 要求 connectionConsent；Select 要求真实枚举中存在的 Connected 目标以及用户主动选择 kind。TCP、USB、设备名称和回环地址均不能判定模拟器或真机。
- Deploy 要求 deploymentConsent，仍受 HAP/签名门禁。启动、截图、输入及停止不会因用户 consent 或客户端退出 0 被标为成功。
- Cancel/Disconnect 释放 ArkDock 的本地选择，不调用 `hdc kill`、全局断开、提权、重启或修改设备。当前没有自有设备进程或客户端需要终止。

## 发现门禁

当前 Discover 返回 `HDC_SERVER_LIFECYCLE_UNVERIFIED`，不创建客户端线程或虚假的 discovering 状态。原因包括：

1. 已有 `windows_process.rs` 将 stdout 与 stderr 写入同一日志。该文件不能用作设备列表的权威 stdout。
2. HDC 普通客户端可自动启动共享 server；尚未核验本机商业工具的 server 生命周期、归属、兼容性及取消策略。
3. 参考 HDC 源码中的 `-p` 是未公开选项，本次不调用它来规避 auto-start。

这些条件解决后，实际枚举应在有界后台线程中执行，先绑定当前身份和独立请求代次，分离 stdout/stderr，再等待真实回执并严格解析 stdout。取消只终止该线程持有的自有 Job/client；共享 server 不能按名称终止。旧身份的结果必须丢弃。当前没有开放前端注入目标列表的路径。

## 官方来源与命令

参考源码不等于 SDK 26 或具体设备的兼容证明。本次没有移植官方 C++ 代码或分发二进制。

| 来源 | 固定参考提交 | 作用 |
|---|---|---|
| [developtools_hdc_standard](https://github.com/openharmony/developtools_hdc_standard/tree/08924fba62069f6716e71266391092271faf10a3) | `08924fba62069f6716e71266391092271faf10a3` | host/main.cpp、server.cpp、translate.cpp：参数重建、自动 server、列表格式与 HDC 参数 |
| [OpenHarmony docs](https://github.com/openharmony/docs/tree/f41b9345badd47c7ab0c263344cd7f4b5a549afb) | `f41b9345badd47c7ab0c263344cd7f4b5a549afb` | aa/bm、HDC、UItest、签名 Profile |
| [testfwk_arkxtest](https://github.com/openharmony/testfwk_arkxtest/tree/a9a8b4700b0b79ee0057f36de6e351d7e647a656) | `a9a8b4700b0b79ee0057f36de6e351d7e647a656` | uitest/server/server_main.cpp 与 input/ui_input.cpp |
| [device_qemu](https://github.com/openharmony/device_qemu/tree/434e755fdb46ea93df016b56eb4f3033823d36d4) | `434e755fdb46ea93df016b56eb4f3033823d36d4` | 板级仿真与系统镜像依赖范围 |

以下是受限计划，不是已经执行的命令或安装授权：

| 操作 | 官方参数 |
|---|---|
| 枚举 | `hdc list targets -v` |
| 安装 | `hdc -t TARGET install ABSOLUTE.hap` |
| 启动 | `hdc -t TARGET shell aa start -a ABILITY -b BUNDLE -m MODULE` |
| 停止 | `hdc -t TARGET shell aa force-stop BUNDLE` |
| 指定包信息 | `hdc -t TARGET shell bm dump -n BUNDLE` |
| 截图 | `hdc -t TARGET shell uitest screenCap -p /data/local/tmp/arkdock_NONCE.png` |
| 拉取截图 | `hdc -t TARGET file recv SAME_REMOTE OWNED_LOCAL.png` |
| 点击 | `hdc -t TARGET shell uitest uiInput click X Y` |
| 滑动/拖动 | `hdc -t TARGET shell uitest uiInput swipe FROM_X FROM_Y TO_X TO_Y VELOCITY`；drag 使用相同参数 |
| 已聚焦位置输入 | `hdc -t TARGET shell uitest uiInput text TEXT` |

参数参见 [HDC translate.cpp](https://github.com/openharmony/developtools_hdc_standard/blob/08924fba62069f6716e71266391092271faf10a3/src/host/translate.cpp)、[aa 工具](https://github.com/openharmony/docs/blob/f41b9345badd47c7ab0c263344cd7f4b5a549afb/en/application-dev/tools/aa-tool.md)、[bm 工具](https://github.com/openharmony/docs/blob/f41b9345badd47c7ab0c263344cd7f4b5a549afb/en/application-dev/tools/bm-tool.md) 与 [UItest 指南](https://github.com/openharmony/docs/blob/f41b9345badd47c7ab0c263344cd7f4b5a549afb/en/application-dev/application-test/uitest-guidelines.md)。

安装默认不附加替换、降级或自动授权权限选项。前端不能传任意 bundle、Ability、产物路径或 Shell。未来执行者必须从当前可信工程与实际构建收据读取这些值，核验安装文件摘要及自有截图目标目录。

HDC 会重新拼接 argv，设备 shell 会解析命令。主机结构化参数不能解决第二层 Shell 注入。本次计划仅允许安全 token；路径限定无空格、无 Unicode 的绝对 Windows 盘符路径，文本限定 ASCII token。中文、空格、组合键、完整 IME 和多屏输入不开放。UItest 当前参考源码拒绝零坐标，速度限定 200 至 40000 px/s；这些限制不能静默转换为其他输入。

## 部署、图像和模拟器阻塞

- **HAP 构建缺失**：现有 `PreviewBuild` 是预览构建，不生成可安装的 HAP。需要独立可信工程打包流程、产物定位、摘要和构建代次收据。
- **签名缺失**：需要实际验证签名链、有效期、bundle 对应关系及调试设备许可，不能以签名文件存在或 `signed: true` 替代。不能读取或导出私钥、口令等凭据。参考 [HarmonyAppProvision](https://github.com/openharmony/docs/blob/f41b9345badd47c7ab0c263344cd7f4b5a549afb/en/application-dev/security/app-provision-structure.md)。
- **部署授权缺失**：connectionConsent 只允许连接/发现。实际 install 还需要绑定准确目标、workspace/session/build/generation、HAP 摘要、bundle/module/Ability 的短时单次授权；项目切换、重建或设备断开必须失效。
- **输入范围缺失**：screenCap 捕获整块 display，画面不自动证明目标 bundle 前台。截图需单独授权；输入需验证前台应用、display、尺寸和帧新鲜度，以免发往设备其他应用。
- **回执语义不足**：参考 HDC main.cpp 最终返回 0；UItest 输入函数也可能打印异常后返回成功退出码。安装、启动、输入回执、实际显示帧和完成须分别判断。
- **设备帧未接入**：后续真实 PNG 传输必须拥有独立 deviceRunId、target/bundle/display、原生 scope、sequence、捕获时间、尺寸/载荷上限及取消状态；不得拼造 Previewer 帧头。
- **完整模拟器缺失**：QEMU 参考仓提供内核/板级适配；标准系统指南还依赖另外获取并构建的系统镜像。尚无已核验的完整商业 HarmonyOS 应用虚拟设备、图形/输入/HDC 服务与合法安装来源；保持 `FULL_EMULATOR_RUNTIME_UNAVAILABLE`。不能自动编译 QEMU、下载镜像或将 Previewer 当模拟器。参考 [QEMU README](https://github.com/openharmony/device_qemu/blob/434e755fdb46ea93df016b56eb4f3033823d36d4/README.md) 和 [Arm 标准系统指南](https://github.com/openharmony/device_qemu/blob/434e755fdb46ea93df016b56eb4f3033823d36d4/arm_virt/linux/README.md)。

## 用户验收步骤

全部步骤由用户执行，本次无自动测试或运行报告。

1. 选择已有合法 SDK 与可信工程，检查设备运行状态中的 HDC 路径和摘要；文件观察不能显示已连接、部署就绪或已有真实画面。
2. 未授权连接时尝试 Discover，应看到明确 consent 阻塞；允许连接后应看到 server lifecycle 的具体阻塞，并确认没有启动 HDC 或设备部署。
3. 未出现真实发现结果时不能选择、安装或启动任意设备；不得将唯一设备、USB 或回环 TCP 自动认定为真机/模拟器。
4. 检查 Deploy、Start、Capture 的不同阻塞原因，分别指出 HAP/签名/安装身份/前台与截图授权缺口；不能把 consent 开关变成成功回执。
5. 检查切换工程、重建以及取消/断开会释放旧本地选择；用户既有 HDC server 和其他工具连接应保持正常。
6. 对将来接通的后台客户端，由用户验证 timeout、取消、分离 stdout、迟到响应及真实列表格式。非法日志行、重复目标、超限、非 UTF-8 应拒绝整个结果。
7. 对将来部署/帧/输入接通，用户先完成目标绑定签名和授权，再观察安装、Ability 启动、实际设备画面与对应输入变化；每个阶段独立记录，未观察到的阶段保持阻塞。

本次没有执行以上步骤，所有结果仍是 **NOT_RUN — 等待用户验收**。
