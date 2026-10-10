# 工程发现证据

2026-10-09，Windows 11 原生编译、Rust 测试和实际 UIA 标签采集已执行。
发现与执行信任分离；默认旅程仍为拖动绑定 → 当前工程线索 → 候选确认。
当前没有默认自动关联来源。UIA 选择属性只证明标签被选中，不证明通用应用的活动工程范围。

## 可核验的当前窗口来源

desktop 的 WindowsTabResolver 在不拥有窗口的 backend MTA 线程初始化 COM，
仅读生命周期核验目标 HWND 的 ControlView；前后重验目标 token、PID/创建 FILETIME。
关闭 AutoSetFocus，连接/事务 timeout 各 500 ms；树限制 128 元素、深度 4、遍历期限三秒。
只递归 Pane/Tab/Toolbar/Group，仅读可见且 SelectionItem selected 的 TabItem 的 Name。
不读取 Document/Edit/Text/List、聊天内容、进程内存或凭据；接口不跨 apartment 持有。
只接受元数据中的完整路径，证据 TTL 30 秒。verifiable=true，但 activeScope=false；
无法证明活动范围时，即使只有一个有效候选仍需用户选择。provider 失败会降级到其他候选线索。

实际驱动在同 PID 双窗口中的所绑定窗口放置 selected TabControl：
真实 UIA 返回 entry 模块完整路径、来源和身份，项目仍为空、trusted=false、
phase=selectProject，未自动关联。这个普通窗口 fixture 仅用于窗口/发现验收。

## 只读范围和预算

只访问显式路径、至多 12 层祖先和明确声明模块。应用需 build-profile.json5 +
AppScope/app.json5，模块需 src/main/module.json5。JSON5 文件每个最多 1 MiB；
模块最多 64、每模块目标最多 16；元数据字段最多 1024 bytes。
所有候选包括无效候选，在字符串/证据复制前共享 256 项和 8 MiB 的分配预算。
字节预算包含可变字段和固定记录余量，是拒绝超量输入的保守预算，非 OS 堆测量。
不执行 hvigorfile.ts、包脚本或项目代码；fixture 故意含抛错的 hvigorfile。

显式目录若为 .git / pnpm-workspace.yaml / lerna.json 标记的工作区，且祖先没有应用，
可下探两层，最多 64 目录、512 条目、16 应用。跳过符号链接、依赖、build、
.git 与缓存，规范化后必须仍在根内。超预算明确拒绝并建议选子目录，不悄悄截断。
不下探任意目录或祖先磁盘；不读取 Git 元数据。

srcPath 必须为受限相对路径，配置/模块规范化后不得越界。
工作区/应用完整路径、环境、模块和变体形成独立身份，同名 monorepo 应用和 worktree
不合并。源码文件缩小到其所在模块；应用根可列全部模块/变体。

Windows、WSL UNC/URI、远程身份不互相转换。原始和 canonical 环境两次核验；
Windows POSIX 前缀、WSL 映射到同名本地目录均拒绝。不支持环境保留候选原路径和理由。
当前 Windows WSL UNC/remote 原生执行不支持，不声称 WSL 工作流已验证。

## 决策与信任

automatic_candidate 每次重新核验有效 Windows 本地工程、来源、认证/活动范围、
目标 token、PID/启动 token、会话与 TTL。只有一个当前可靠强候选且没有冲突才可自动关联。
多个弱来源、历史、最近修改时间、标题和手选不升级为可靠证据。
文件登记仍 verifiable=false / activeScope=false；字段匹配不能证明发送方认证。
认证命名管道、启动参数来源和宿主活动工程证明未实现，B2 完整强链路 BLOCKED。

工程选择不授权执行。信任、撤销、历史和根/模块构建配置指纹回归实际通过；
未信任项目不启动脚本。运行期 HWND/PID/创建时间不写入历史。
候选界面保留完整路径、环境、模块、来源、有效性与推荐/拒绝理由，支持搜索与滚动。
新版蓝色玻璃界面和浅/深/系统主题不改变发现或信任决策。

## 验证与来源

Rust discovery、helper 与持久化测试实际执行通过；新增 monorepo 与元数据预算
红/绿日志保留在 artifacts。UIA 当前选中标签候选由真实 WebView2/WinForms 驱动断言。
具体数量、命令及退出码见 [本次验证](verification-2026-10-09.md)。

[Huawei 工程结构](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides-v5/ide-project-structure-V5)、
[工程级 build-profile](https://developer.huawei.com/consumer/cn/doc/doccenter-deveco-studio/ide-hmos-hvigor-build-profile-app)
仅提供结构资料，不证明本机 SDK 协议兼容。
[Microsoft UIA threading](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)、
[IUIAutomation2](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nn-uiautomationclient-iuiautomation2)
用于 COM 和 provider timeout 边界。
tests/fixtures/harmony-app 是只读发现 fixture，不用于真实原生构建验收。
