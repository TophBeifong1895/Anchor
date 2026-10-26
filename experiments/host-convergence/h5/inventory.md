# H5 编辑前盘点与预算

2026-10-26；基点 `fcda48c`，分支 `host/h5-reverse-deps`，worktree `/tmp/anchor-host-h5`。已读 AGENTS.md、文档入口、任务书 H3 遗留、H4 验收与 H5。尚未修改生产代码、测试或围栏。

当前具体方案估算净增 **186 行**，估算区间 **+114～+280 行**，不能承诺满足生产行数净增 ≤ 0。按用户的编辑前 gate 停在此处，交回约束取舍。此处是方案预算，不是所有可能实现都无法达标的证明；没有把估算写成实测。

## 口径与消除方式

- 下列 46 行逐一对应现有围栏计数：服务 → 用例 22、服务 → 适配 23、用例 → 适配 1。基线行号来自 `fcda48c`；一个 import 内多个名称按围栏的一处路径计数。测试、注释、字面量不计。
- 每行标注增减或共用预算编号；同组增量只在预算表累计一次。纯迁移按约 0 行计算，不把原路径删除重复算成收益。
- 不改变围栏的层次分类，不靠相对 import、别名或重导出隐藏上层类型，不把整个 `node_host`、`pilot_host` 重分类为入口。
- Run owner、在线 worker、暂停通知与历史投影保持原调用链。`read_run` 不能被 `run_access::load` 统一替换：前者使用注册 owner，后者只读磁盘。Run 业务围栏继续为 0。
- 小端口由下层声明、组合根注入；实现留在现有事实所有者。移动到用例层的仅是原先已协调 Run 生命周期的函数；共享文件操作与事实存储继续留在服务层。

## 服务 → 用例：22 处

| 编号 | 基线位置 | 实际指向与用途 | 消除方案 | 预计生产净增减归属 |
| --- | --- | --- | --- | --- |
| A01 | `assistant.rs:1` | `run_access`；322、325、383、425 读交接 Run | 将 `handover_predecessor`、`handover_workspace_source` 的跨 Run 协调迁入 Run 用例；Node 经注入端口调用。原 Session/绑定校验、读取顺序与错误文本保持 | G1，共用 +12 |
| A02 | `assistant_plugins.rs:5` | `RunProgress`；授权身份与清理输入 | 只含公开快照字段的 `RunProgress` 移至中立契约；从内部 record 转换仍留在 Run 门面，禁止业务直接访问内部 record | G1，已合并 |
| A03 | `assistant_plugins.rs:6` | `run_access::{self, RunView}`；Graph 租约、当前/最新 Turn、更新投影 | Graph 租约唯一实现下沉到现有授权服务，删掉两层转发；`active_bindings`、`latest_policy`、`status` 的 Run 协调归 Run 门面，继续调用原政策事实函数 | G1，已合并 |
| A04 | `channel_supervisor/management.rs:5` | `ApplicationError`、ChannelBinding/Callback/Port/RuntimeState、PLATFORM、channel_plan | 渠道契约、归一化、唯一 bot 判定归渠道服务；服务返回渠道错误，由用例边界映射，保留分类和文本 | G3，+22 |
| A05 | `channel_supervisor/management.rs:371` | `channel_management::NOT_A_BOT` | 常量随唯一 bot 判定归渠道契约，调用处用同一值 | G3，已合并 |
| A06 | `conversation_resources.rs:4` | `run_access::{self, RunView}`；删除时保存与检查作用域引用 | 此模块的调用方均在用例层；将其协调删除、保留引用的用例整体迁到 `application/conversation_resources`，底层原文件操作保持调用 | G1，已合并 |
| A07 | `conversation_resources.rs:5` | `ApplicationError`、`RunApplication`；同上 | 随删除用例迁移，不另建错误与转发层；原磁盘观察与 metadata 错误顺序保持 | G1，已合并 |
| A08 | `durable_write.rs:238` | 默认配置 `RunApplication::new`，只取 durable_input_workspaces | 清理函数接收惰性的工作区枚举回调，由入口注入现有 application 能力；保留先扫目录、后枚举、错误计入 failed 的顺序，不提前执行枚举 | G4，+7 |
| A09 | `goose_acp/pilot_interaction.rs:1` | PluginInstallPrecondition、PluginUninstallPrecondition | 原生表单/确认适配整体迁到 `pilot_tools/interaction`，保留前置条件的所有者与再校验；Goose 经端口进入适配器 | 纯迁移约 0；调用边界计 G8 |
| A10 | 同文件 `:82` | `application_reason` 参数 ApplicationError | 随交互适配迁移，保留 Missing 与三类原因文本映射 | 0，G8 共用 |
| A11 | 同文件 `:83` | ApplicationError import | 同上 | 0，G8 共用 |
| A12 | 同文件 `:227` | GraphManagementError::Conflict | 随交互适配迁移，保留内容变化时的冲突分支 | 0，G8 共用 |
| A13 | 同文件 `:279` | 卸载表单 PluginUninstallPrecondition 参数 | 随交互适配迁移，原表单与授权内容不变 | 0，G8 共用 |
| A14 | 同文件 `:510` | render_coverage 的 ImpactCoverage 参数 | 随交互适配迁移，完整/部分/未知的显示保持 | 0，G8 共用 |
| A15 | 同文件 `:511` | ImpactCoverage import | 同上 | 0，G8 共用 |
| A16 | 同文件 `:805` | plugin_candidates::load_fact | 随交互适配迁移，候选事实仍在原步骤读取 | 0，G8 共用 |
| A17 | `local_inputs.rs:3` | RunProgress；verify、mounts 输入 | 使用 A02 的中立快照契约，转换与 Run 所有权不下放 | G1，已合并 |
| A18 | `node_host.rs:100` | OnceLock<RunApplication> | 改存 Node 所需端口；由原 application 实现在线查询/通知，组合根注入。端口含 execution_record、dispatch_inputs/keys、wait_pause、Session successors、history_allowed、history_projection；A01 的交接操作另外计 G1 | G2，+70 |
| A19 | `node_host.rs:189` | run_application 返回整个 RunApplication | 删除返回整个用例对象的入口，调用对应小端口；合并 load_run 中相同错误映射 | G2，已合并 |
| A20 | `node_host.rs:192` | fallback RunApplication::new(root, root) | 删除下层默认构造。独立宿主与平台宿主各从组合根提供能力；保留没有已绑定暂停通知时的原行为，不能把后注入 owner 当成原 owner | G2，已合并；配置项计 G13 |
| A21 | `node_host/conversation.rs:228` | HistoryTools.application | 改持历史投影小端口；read_run → metadata → project_run 的顺序及三种错误前缀在所有者实现中保持，trace 读取仍先执行 | G2，已合并 |
| A22 | `pilot_host.rs:716` | development_dependencies::release_development_scope | 唯一释放函数归 Pilot 资源所有者 `pilot_resources`，删除旧路径，不保留兼容转发；Library tombstone 在私有树清理之前 | G12，+1 |

## 服务 → 入口适配：23 处

| 编号 | 基线位置 | 实际指向与用途 | 消除方案 | 预计生产净增减归属 |
| --- | --- | --- | --- | --- |
| B01 | `goose_acp/pilot.rs:5` | PilotTools 具体类型 | Goose 声明工具/原生交互端口，PilotTools 实现；Goose 只接受注入端口和既有 ToolPort，不接触具体适配器 | G8，+35 |
| B02 | 同文件 `:640` | pilot_tools::authority | 原生确认的识别由端口回答，继续复用唯一权限表 | G8，已合并 |
| B03 | 同文件 `:641` | ToolAuthority::Confirm | 同上，Goose 不认识适配层枚举，保留 disclosure → confirmation → Plugin → builtin 顺序 | G8，已合并 |
| B04 | `goose_acp/pilot_interaction.rs:2` | PilotTools、workspace_directory | 随 A09 的原生交互适配迁移，Goose 经 B01 端口调用 | 纯迁移约 0；G8 共用 |
| B05 | 同文件 `:37` | PILOT_TOOLS | 同上，仍复用一份工具定义与授权表 | 0，G8 共用 |
| B06 | `goose_acp/pilot_interaction/channel_management.rs:17` | configuration_arguments | 随交互子模块迁移到适配器内部，参数解析只有一份 | 0，G8 共用 |
| B07 | 同文件 `:35` | enable_arguments | 同上 | 0，G8 共用 |
| B08 | `goose_tool_context.rs:48` | channel_tools::SEND_TOOL | 共享工具名移到中立工具契约，保留字符串与序列化 hash 输入字节 | G6，约 0 |
| B09 | `node_host/assistant.rs:457` | read_reply_images_for_invocation | Host 保存回复的读取校验归回复事实服务；channel_tools 的写入和 Node 结算共用该实现，保留字节与校验顺序 | G7，+2 |
| B10 | `node_host/preparation.rs:110` | SpillDirectory 构造 | 注入工具装配端口；现有 sandbox 工具与 channel wrap 的装配在组合根提供的实现中完成，不搬整个 node_tools 绕分类 | G5，+28 |
| B11 | 同文件 `:123` | NodeTools 构造 | 同上，内层插件端口、sandbox、workspace、授权、取消等仍传原值 | G5，已合并 |
| B12 | 同文件 `:124` | channel_tools::wrap | 同上，原 eligibility 判定与 wrap 顺序不变 | G5，已合并 |
| B13 | 同文件 `:141` | command_timeout_ceiling | 同上，预算上限仍在同一阶段计算，配置不在 Node 重复读取 | G5，已合并 |
| B14 | 同文件 `:149` | RUN_TOOL_NAME | 使用 B08 的共享契约 | G6，已合并 |
| B15 | 同文件 `:150` | READ_TOOL_NAME | 同上 | G6，已合并 |
| B16 | 同文件 `:151` | EDIT_TOOL_NAME | 同上 | G6，已合并 |
| B17 | `pilot_host.rs:10` | PilotTools；PilotAdmission 的工具与执行能力 | PilotService 接收 Turn 执行端口和所需路径/bwrap 参数；具体工具冻结/Goose 执行由适配层实现，三个 API 生产构造入口注入 | G9，+35 |
| B18 | 同文件 `:855` | PilotTurnContext::new | execute_turn 的完整装配随 B17 迁移，仍在执行注册之后启动，不进入 admission 全局 gate | G9，已合并 |
| B19 | 同文件 `:875` | frozen_turn_tools | 同上，冻结、取消、部分启动与 shutdown 清理顺序保持 | G9，已合并 |
| B20 | `pilot_resources.rs:29` | pilot_tools::session_workspace | 唯一工作区派生函数归资源所有者，所有调用改路径；hash 输入、目录字节不变，不保留转发 | G11，+1 |
| B21 | `tool_host.rs:1220` | node_tools::RUN_TOOL_NAME，MCP 防遮蔽 | 使用 B08 共享契约 | G6，已合并 |
| B22 | 同文件 `:1221` | channel_tools::SEND_TOOL，MCP 防遮蔽 | 同上 | G6，已合并 |
| B23 | 同文件 `:1222` | channel_tools::IMAGE_TOOL，MCP 防遮蔽 | 同上 | G6，已合并 |

## 用例 → 入口适配：1 处

| 编号 | 基线位置 | 实际指向与用途 | 消除方案 | 预计生产净增减归属 |
| --- | --- | --- | --- | --- |
| C01 | `application/plugin_candidates.rs:357` | test_plugin_candidate 的 PilotTurnContext 参数，只用 access/sandbox/workspace | 改收已有 DependencyAccess、BubblewrapSandbox、workspace 参数；不建第二份上下文，不向用例传整个适配器 | G10，+5 |

`application/pilot_sessions.rs:111` 的 PilotTools 构造位于 `#[cfg(test)] mod tests` 内，不是额外的生产调用；不能把它计入生产预算。围栏仅作词法扫描，本盘点同时核对实际调用，未将这一扫描限制当作绕过手段。

## H3 遗留与行数账本

G13 除 A20 外还包括 `execution.rs:64–67` 的默认 reader 和 `durable_write.rs:238` 的默认 application。入口已有真实配置；独立宿主也从入口构造/注入实际能力，下层不再以两个相同根和默认值伪造 configuration。

RunApplication 只保留现有 data/catalog 根、Library 覆盖和 bwrap，不再保存整份 HostConfiguration。不把删掉的配置副本换成按调用重建的副本。`check_store` 保留 executor 校验、原创建与 canonicalize 操作，只删恒等比较；sandbox 接口改收现有类型化路径/参数。API 配置连接测试生产调用也改用 bwrap 参数；现有测试只改构造和模块路径。

| 预算组 | 依据与替代/删除路径 | 净增减点估计 | 区间 |
| --- | --- | ---: | ---: |
| G1 Run 相关服务职责 | 交接用例迁移与端口约 +20，政策/契约/资源迁移 import 约 +8；删除 assistant_plugins 两个租约转发、门面 re-export 等约 -16 | +12 | +4～+20 |
| G2 Node 所需 Run 小端口 | node_host.rs:100,189–208；conversation.rs:228,285–301；kernel_runs.rs:402,442,552–585；session_calls.rs:20–65。端口/实现/组合根接线增加；合并重复 load_run 映射、删除通用 reader 入口；history 用例顺序整体移入实现 | +70 | +45～+90 |
| G3 渠道契约与错误 | channel_management.rs:15–189 契约迁移约 0；渠道自身错误与用例边界唯一转换增加。management.rs:725–727 的 storage 分类仍有效，不能当冗余删 | +22 | +16～+30 |
| G4 启动清理注入 | durable_write.rs:202–250 及两个入口：惰性回调签名、入口接线，删除默认 application 构造 | +7 | +4～+10 |
| G5 Node 工具装配 | preparation.rs:108–153 的原装配移动约 0；新增小契约、工厂与组合根接线，删除服务直接构造适配器 | +28 | +20～+40 |
| G6 共享工具名 | node_tools.rs:21–23、channel_tools.rs:32–33 唯一定义移动，所有使用改路径，不留转发 | 0 | 0～+2 |
| G7 回复图片事实读取 | channel_tools.rs:43–120 原实现移动约 0；新增归属 import/模块声明，禁止复制验证 | +2 | 0～+4 |
| G8 Goose 交互端口 | pilot.rs:187–235、541–550、636–665；interaction 整体移动约 0；端口与实现增加，删除原 definitions 拼接和直接 authority 判断 | +35 | +30～+45 |
| G9 Pilot Turn 执行端口 | pilot_host.rs:108–118、329–344、509–530、832–954；execute_turn 123 行移动约 0；新增端口/接线，删除或吸收 pilot_tools::for_turn 的约 5 行和直接 bwrap 访问链 | +35 | +30～+50 |
| G10 候选测试参数 | plugin_candidates.rs:354–362、451–456 及一处生产调用；改接现有三个参数，删本地 access 派生 | +5 | +3～+10 |
| G11 工作区归属 | pilot_tools.rs:64–70 → pilot_resources，唯一实现迁移 | +1 | 0～+2 |
| G12 开发 scope 释放 | development_dependencies.rs:182–198 → pilot_resources，原 tombstone/私有树清理一起迁移 | +1 | 0～+3 |
| G13 H3 配置副本删除 | application.rs:86、269–305、335–357：删 HostConfiguration 副本及恒等比较，保留建目录；包含参数接线成本，默认构造删除不与 G2/G4 重复记收益 | -24 | -30～-18 |
| G14 Plugin 转发清理 | plugin_library.rs:147–154 的 install_directory 只有测试调用，可用 cfg(test) 保留测试构造，生产减少 8 行；没有发现完全无调用的转发方法 | -8 | -8 |
| 合计 | 纯迁移不计大额删行收益，不按压缩格式凑行数 | **+186** | **+114～+280** |

这是编辑前的函数级估算，未生成实现、未用修改断言换取收益。按该预算在同一范围实现 ≤ 0，还缺约 114 行、点估计约 186 行的确定删除来源；仅删除当前识别的冗余不足。

## 删除机会核对

- `plugin_library` 的生产/测试转发均有调用；`Catalog::view_path`、install_lease、try_install_lease 已受 cfg(test) 保护。`install_directory` 可补同类保护，不能删除测试还需要的入口或改测试断言。mcp_servers、review/stage/install_locked、development_scope/release 均有生产调用；resolve_mcp_servers 两种形式也各有调用。
- `assistant_plugins::graph_lease` 与 graph_read_lease 是可删除的上层转发；底层租约实现必须保留唯一一份，路径与锁语义不变。
- `application_reason` 承接 Missing 与原因文本映射；直接删除会改变工具输出。Channel 的 storage 转换仍用于错误分类；不能只为行数删除。
- `RunApplication::execution_record`、dispatch_keys 等在组合根仍有调用；换成端口后并不自动都成为可删函数。部分只被 Node 使用的方法可把正文迁入端口实现，不把“迁移”再计一次删除。
- RunView::needs_attention 等虽没有生产调用，现有断言仍验证其门面契约，不将这类测试固定的接口删掉凑预算。

## 交回取舍

当前未选择任何一种，也未登记命名例外。需要用户在审阅清单后决定下一步：

1. 继续保持 ≤ 0，先要求进一步只读寻找同项确定删除来源；当前方案不进入实现。
2. 保持清零与行为约束，明确允许本项正增量预算，再按实测收敛。
3. 明确允许扩大同项删除范围，先列出额外删除清单和行为影响，再决定是否实现。

这些选择不自动授权修改任何现有断言、持久事实、权限或并发/恢复语义。未把遗留引用改成命名例外绕开预算。
