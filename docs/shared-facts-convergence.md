# 共同事实与实现收敛

本页描述共同规则的当前归属与允许的上下文差异。2026-10-16 的实现核查覆盖 HTTP、Pilot 与 Web；原清单及新增的删除确认、历史拓扑、角色默认值和损坏插件撤销问题已接入下表的共享实现。实际验收结论、未覆盖边界与证据统一见 [当前验收与待办](pilot-development-plan.md)，不据此宣称全仓或所有真实 Provider 已验收。

## 当前规则与唯一实现

| 范围 | 权威实现与调用入口 | 保留的边界 |
| --- | --- | --- |
| 插件目录与基础健康 | [InstalledPluginCatalog](../rust/anchor-runner-host/src/application/plugins/catalog.rs) 统一目录发现、轻量元数据和依赖视图观察；HTTP、Session、Pilot 列表消费同一结果，坏清单保留为不可用项 | 基础定义有效性不证明代码正文可读、执行绑定、权限或连接成功；Session 可叠加上下文拒绝。完整定义按详情、挂载启用或执行需要读取，管理列表不生成 digest |
| 插件设置完整交互 | [PluginSettings](../apps/web/src/PluginPanel.tsx) 统一开关、已选优先排序、筛选、搜索、加载、重试和反馈；Graph、Pilot、Library、Run 提供数据与写入适配 | Graph 保存草稿引用，Session 写持久挂载策略，Run 显示冻结绑定记录；历史记录不因当前目录变化而重写 |
| 插件挂载与撤销 | [Session 挂载入口](../rust/anchor-runner-host/src/api.rs) 对启用执行定义与依赖校验；撤销只验证 Session 归属、生命周期及 required 约束，不要求被撤销插件仍健康 | 缺失或清单损坏的已开启引用仍可见并可撤销；权限和发布锁不因关闭操作放宽 |
| Graph 发现、读取与校验 | [Graph 应用用例](../rust/anchor-runner-host/src/application/graphs.rs) 统一 bundle 有效性、读取及作者/私有诊断分类；HTTP、Pilot 只投影格式与脱敏 | 配置 Graph 别名仍由部署拥有；损坏目录不作为可运行 Graph 暴露，作者输入错误与私有 Library 错误保留诊断边界 |
| Run 状态与控制投影 | [RunProjection](../rust/anchor-runner-host/src/application/run_projection.rs) 使用 RunStatus、RunLiveness 和原控制前置条件；Run 列表、详情、时间线、子调用和 Pilot 查询复用，Web 共用 [展示](../apps/web/src/runPresentation.ts) 与 [控制组件](../apps/web/src/RunControls.tsx) | 投影只是当次观察，控制仍校验当前身份、scope、lease 和执行登记；渠道可叠加等待消息等上下文。缺投影不由 Web 猜测控制权限 |
| 历史运行拓扑 | Run 详情的 definition 仅从持久 record.snapshot 生成，运行画布使用该只读投影，包含已展开的 inline 模块 | 它是展示拓扑，不是完整作者定义，不含 layout、脚本、指令、输入或 host 参数。当前 Graph 与未保存草稿不影响历史 Run |
| Graph 删除确认 | [Web 确认界面](../apps/web/src/GraphActions.tsx) 与 Pilot 在确认前捕获已有删除前置条件，交给同一受保护删除用例重新核对资源版本 | 变化或重建后旧确认失败；HTTP 保留不带 If-Match 的显式无条件兼容接口，交互确认必须使用条件删除；运行/引用等删除条件始终由后端校验 |
| 新角色模型默认值 | Web 在创建时读取 ModelConfiguration 的 execution.default_reference，经 [共享角色构造](../apps/web/src/agentAuthoring.ts) 服务普通添加和首次并行区域 | 用户显式模型与旧别名保留原义；设置读取失败不写入猜测的默认引用，异步响应不得覆盖更新后的草稿 |
| 文件下载 | Run 文件和 Pilot 附件复用 [DownloadButton](../apps/web/src/DownloadButton.tsx) 与 apiBlob，统一 Bearer、取消、重复点击及失败反馈 | 文件地址、归属、不可变版本和授权仍由对应服务所有；Markdown 内嵌资源的鉴权另列未覆盖边界 |
| 新 Pilot 默认配置 | Web 与 Responses 共用 [Pilot 创建用例](../rust/anchor-runner-host/src/application/pilot_sessions.rs)，统一默认 network 挂载 | 通用 SessionStore::create 仍服务不同类型会话；不修改已有 Session 或当前 Turn 的冻结权限 |
| 行操作菜单 | Graph、Pilot 会话复用 [ActionMenu](../apps/web/src/ActionMenu.tsx) 的定位、关闭、键盘、焦点和主题 | 菜单内容、删除范围、确认事实与权限仍由相应入口负责 |

## 执行与事实边界

RunStatus::is_terminal 和 RunLiveness::is_quiescent 继续是后端终态与静止判断；RunApplication::control 仍是 HTTP、Pilot 和渠道控制的共同用例。查询投影不保存新的运行事实，也不建立另一套执行或恢复引擎。

插件目录的 definitionValid 表示清单及声明路径结构的基础有效性；dependenciesReady 表示当前依赖视图就绪，定义无效时为未知。contextAuthorized 与 connectionVerified 没有相应上下文或真实连接证据时保持未知。available 不能解释成已授权、连接成功或业务已验收，Session 可依据自己的上下文进一步否决。

前后端通过契约共享事实，Rust 与 TypeScript 不要求共用源文件。客户端草稿检查与服务端最终校验可以分工，但规则口径必须一致。Graph、Session、Turn 与 Run 的不同生命周期、权限和历史快照不因本次收敛合并。

## 继续验收的方法

每个切片核对共同规则的唯一实现、调用入口、允许的上下文差异及旧分支移除；同一规则改一处后所有入口应共同生效。代码行数、组件同名或单个页面分别可用不能替代跨入口一致性。权限撤销、损坏资产、历史快照与确认期间并发变化均应纳入回归。

本轮不实施 [既有收敛方案](convergence-development-plan.md) 的 Run 索引、兼容退役政策、预算或度量能力。凭据、环境与安装/卸载已有共享用例的抽查不能代替它们各自的完整真实业务验收。
