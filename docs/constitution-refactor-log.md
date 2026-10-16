# 宪法内核重构：工作记录

分支 `constitution-core`，worktree `/home/mansteinl/code/anchor-constitution-core`，基于 master `fc558e2`。设计依据：[docs/anchor-constitution.md](anchor-constitution.md)；概念验证：[experiments/constitution-poc](../experiments/constitution-poc/README.md)。

本文记录每一刀的实际结果、数字和语义变更。它是分支内的工作记录，不是进度台账。

## 纪律

1. 测试是契约。修改既有断言必须在"语义变更"一节登记原因。
2. 每一刀结束时编译通过、测试通过。
3. 分支上不背兼容包袱；生产数据迁移在合并时单独处理。
4. 度量：同一规则的独立判断点数量、核心决策代码量、测试结果。代码行数不是目标。

## 基线

| 范围 | 结果 | 说明 |
| --- | --- | --- |
| `anchor-runtime` | 119 通过 | 未改动的 master |
| Host、GraphHost、Runtime 全部目标 | 1227 通过，12 失败，112 忽略 | 未改动的 master，`--no-fail-fast` |
| 上述 12 项失败 | 复跑后全部通过 | 环境原因：新 worktree 未构建 `anchor-sandbox-network`，网络相关测试无法启动。构建后复跑 API 测试 197 项全部通过 |
| K0 + K1 之后，Kernel、Runtime、Host、GraphHost 全部目标 | 1257 通过，0 失败，112 忽略 | 43 个测试目标。1257 = 基线 1239 项（含复跑后通过的 12 项）+ 新增 18 项（内核 17、差分测试 1） |

## 只读检查的发现

- 旧内核的核心是 `GraphRunner::run`，约 770 行的单个函数，一个循环里同时处理生命周期、选节点、拼输入、按节点类型分支、映射 9 种执行结果到 10 种 Run 状态、选路由、冻结产物、写边决定。
- 停止和暂停的检查在循环里出现 6 处。
- `GraphRunRecord` 约 20 个字段，其中约 9 个是簿记：Run 的当前状态需要从计数器和映射里拼出来。
- 内核内部结构已泄漏到内核之外：`anchor-runtime` 之外有 16 个文件读 `record.cursor`，21 个读 `invocations`，16 个读 `graph_calls`，25 个使用 `NodeCompletion`，47 个按 `RunStatus` 分支。替换内核必然牵动 Host。
- 带条件路由的数据流语义（边被判定为不走之后，如何关闭下游节点和环）属于 Graph 语言本身，不是推模型带来的复杂度。新内核不会消除它，只会把它移到 Graph 校验器（实例层）。

## K0：新内核 `anchor-kernel`

独立 crate，不依赖 Graph、模型或协议。依赖只有 serde、serde_json、thiserror。

- 六种原语：资源、提交、裁决、效果、任务、测量。
- `submit` 是唯一的改变入口：裁决、记录、结算、测量。
- 授权是端口（`Authority`），内核只询问，不实现。
- 存储由事实所有者提供（`Store`）：内存存储与文件存储。没有全局日志。文件存储先持久化记录、再替换状态；记录携带结果资源，所以"记录已写、状态未写"的崩溃窗口靠读取恢复，不重新裁决。
- 任务生命周期是内核定律：任何任务上，人可以 `stop`、`resume`、`abandon`；等待子任务或未知效果时，由宿主代码 `resume`。等待中的任务不开放空位、拒绝 Agent 的提交。这一条替代了旧 Runner 循环里的 6 处停止和暂停检查。
- 测试 17 项，覆盖 PoC E1 的全部行为，以及 PoC 没有覆盖的持久化：重启后历史完整、崩溃窗口恢复、残缺的最后一行记录被忽略、资源 id 不能逃出存储根目录。

## K1：Graph 作为 Run 的校验器

`anchor-runtime/src/graph/validator.rs`。Run 是内核的任务资源，状态是冻结的 Graph 及其进度。校验器只做两件事：`open` 说明当前接受哪个节点调用（空位 `节点#调用序号`），`judge` 应用提交的结果、选路由、判断 Run 是否结算。它不调用任何人，也不处理停止、暂停、恢复、放弃。

这一刀复用了现有 Graph 语义函数（`next_node`、`propagate_inactive_edges`、`select_route`、`refuse_module_activation`），只替换驱动方式，旧 Runner 一行未改，作为对照组。暂不支持 fanout/join 与 Op.call，`run_resource` 对它们明确拒绝。

**差分测试**：同一个随机 Graph、同一份确定性路由脚本，分别交给旧 `GraphRunner` 和"新内核 + Graph 校验器"，比较派发序列（节点、调用序号、输入产物）、最终 Graph 事实（结果、边的决定及其序号、轮数计数、模块激活、停止原因）和结束状态。

| 指标 | 结果 |
| --- | --- |
| 随机 Graph | 10,000 个，2–9 个节点，含自环、反馈环、模块作用域 |
| 一致 | 10,000 / 10,000 |
| 正常完成 / 触发轮数上限 | 7,397 / 2,603 |
| 总派发次数 / 最长单次 Run | 28,398 / 21 |
| 覆盖：模块被激活 / 模块轮数上限触发 / 存在被跳过的路径 | 1,381 / 336 / 8,977 |

差分测试在第 195 个 Graph 上发现过一个新 API 的缺陷：新侧在派发前的状态推进（关闭失效的边）之前就计算了节点输入，带入了一条已失效的边。修正为 `Dispatch` 与输入在同一次推进之后一起产生。

## K2：并行区域（fanout/join）

校验器的推进函数从"返回下一个节点"变成"返回当前开放的全部空位"：串行时一个，fanout 激活后每个未完成分支各一个，所有分支完成后是 join。并行不再是旧 Runner 里那段约 480 行的特殊执行路径（`parallel.rs`），而是"同时开放多个空位"。

- fanout 与 join 是确定性控制节点，只允许宿主代码（`Code`）提交，Agent 不能伪造一次 join。
- 多个分支同时向同一个 Run 提交时，后到者因版本过期被打回，执行者刷新版本后重新提交，按最新状态重新裁决。这不需要修改内核：版本过期本来就是可修正的裁决。
- 校验器不规定波次。旧 Runner 的"一波全部完成才开始下一波"是旧调度方式的副作用，不是 Graph 语义；新模型里是否分波由调度器决定。差分测试的调度器模拟波次，以便逐项比较。
- 暂不支持：模块作用域位于并行分支之内（`run_resource` 明确拒绝）。

**差分测试（并行）**：随机生成"前缀 → fanout → 2–3 条线性分支 → join → 后缀"，后缀带有重新进入整个区域的反馈边、绕过区域的跳跃边，所有节点有轮数上限（分支可能触发上限）。

| 指标 | 结果 |
| --- | --- |
| 随机 Graph | 5,000 个，全部通过 |
| 完成 / 触发轮数上限 / 分支触发上限导致失败 | 3,633 / 971 / 396 |
| fanout 激活次数 / 总派发次数 | 5,048 / 35,958 |
| 版本过期后重新提交 | 11,444 次 |
| 旧侧幽灵调用（S5）/ 新侧迟到被拒（S6） | 220 / 68 |

完成与轮数上限两种结局要求全部 Graph 事实逐项一致。失败结局按 S6 检查：派发序列完全一致；旧侧并入的每个结果，要么在新侧也被并入，要么是新侧历史中带产物证据的迟到被拒提交，两者不重叠、无遗漏。

## K3：Op.call

调用节点不再是旧 Runner 里约 230 行的结果矩阵，而是映射到已有的概念：

| 旧 | 新 |
| --- | --- |
| `WaitingCall` 状态 | 任务进入内核的"等待子任务" |
| 子 Run 结束后宿主重跑父 Run | 宿主代码提交 `resume`（内核定律允许宿主代码释放等待子任务的任务） |
| 端口返回的六种结果组合 | 宿主代码提交四种之一：已绑定、已完成、失败、未知 |
| 游标记住"这次调用还在进行中" | 从事实推出：已计为开始、尚无结果的调用即进行中，恢复时沿用同一序号，不重复检查轮数上限 |
| detach 不转发业务结果（Runner 构造输出） | 校验器自己构造输出，提交中的业务结果被忽略，结构上不可能泄露 |

同一个调用身份只能绑定一个子 Run，换绑被拒绝。调用节点只能由宿主代码提交。

**差分测试（调用）**：随机串行 Graph 中把一到两个节点改为 wait 或 detach 调用。旧侧使用按调用身份哈希决定结果的脚本化端口，等待中的父 Run 由宿主重新进入；新侧由宿主代码按同一哈希提交，子 Run 结束时由宿主代码释放等待。比较项在原有基础上增加了 `graph_calls` 的全部字段。

| 指标 | 结果 |
| --- | --- |
| 随机 Graph | 5,000 个，全部一致 |
| 等待后完成 / detach | 1,704 / 1,485（detach 无一泄露子 Run 业务结果） |
| 调用失败 / 接纳未知 | 497 / 383 |

差分测试在此过程中发现两个新侧缺陷，均已修正：

1. 已绑定子 Run 的调用在父 Run 恢复后被分配了新的调用序号，丢失了绑定。修正为"已开始、无结果的调用即进行中"。
2. 上述判断最初放在 `next_node` 之后；带自环的调用节点因自环边尚未判定而被 `next_node` 跳过。修正为先检查进行中的调用。

## K0.1：内核修订与中断事实

按[审查决定 D1–D5](constitution-preflight-decision.md)完成限定范围实现，单独提交审查；**H1–H6 尚未开始**。Host、旧 Runner 和现有内核测试断言均未修改。

- **D1**：选择从历史求和；两种 Store 的 `load` 共用成本投影，`spent` 为所有记录 `stamp.cost` 的饱和和，不再依赖版本化快照。打回、拒绝、版本过期、待定及已结算后的拒绝都计费，计费本身不推进版本。版本过期后以 `cost=0` 重提同一工作；超预算仍立即结算失败，结算推进版本。待定批准写入目标任务时的观察记录也遵守预算。
- **D2**：换行闭合才是完整记录；追加前截去尾部未闭合字节，保留完整记录原始字节。任何闭合坏行（包括末尾坏行、空行和无效 UTF-8）均报 `Corrupt`；末尾合法 JSON 若没有换行，也不算已提交记录。崩溃窗口仍从完整记录恢复，不重新执行。
- **D3 / D5**：Code 的 stop 必须有 Observation；人的等待只能由人 resume。等待中所有身份的非生命周期提交由内核拒绝并计费。等待中的非生命周期提交统一返回 `TaskWaiting`；若该次成本同时超预算，D1 仍将任务结算为预算失败，不应用节点内容。
- **D4**：可选 `NodeSubmission.interruption` 仅 Code 可携带，Agent 显式传 `null` 也拒绝。沿用既有规范中断 completion 与普通路由，写入 `RunResult.interruption`，保留现有并行区域边界；普通 Agent 提交序列化省略空中断字段。校验器不冻结或读取产物，`ArtifactKind::Interruption` 的执行者义务留给 H4；Host 的 suppressed 与投递账本未迁移。

原三个反例已成为内核正式回归，并保留隔离旧源码上的失败证据；另新增迟到 Code 结果、全部裁决计费、重试、损坏历史和中断校验测试。三组差分只补构造字段 `interruption: None`，断言和 20,000 例规模均未改。九项检查、实际命令和结果见[本轮验收产物](../experiments/constitution-host-preflight/k01/README.md)。

内核依赖、Record 格式、Waiter 与 Phase 变体均未增加。原预检目录保留为回归，原始失败 JSON 保持历史身份。**已知未决项**：崩溃后宿主重复提交的去重/重复计费本轮不处理；从历史求和的读取开销未做规模验收；固定 Goose、真实 Provider 与 Host 新调度路径不在 K0.1 验收范围。

## 语义变更记录

| 编号 | 场景 | 旧行为 | 新行为 | 理由 |
| --- | --- | --- | --- | --- |
| S1 | 节点提交了非法路由 | 整个 Run 失败 | 打回给提议者修正（`RouteInvalid`） | 宪法：可修正的问题返回提议者，不终止任务 |
| S2 | 所有节点都因轮数上限而停止 | Run 状态为 `Stopped`，可以 resume，但 resume 后立即再次停止 | 结算为 `Escalated`，附停止原因 | 旧状态名义上可恢复，实际上无事可做；新状态如实表达"需要人决定" |
| S3 | 对已 `Aborted` 的 Run 再次 abandon | 幂等成功，返回同一结果 | 拒绝（`TaskSettled`） | 已结算不可逆，不需要为幂等单独设规则；调用方可通过读取状态得到同样信息 |
| S4 | 退役之后再 abandon（PoC E2 发现） | 允许 | 待定：退役属于 Session 指针，尚未迁移 | 需要用户决定 |
| S5 | 并行波次中某个分支触发轮数上限 | 先准备好的兄弟分支被记为"已开始"，但从未执行（幽灵调用） | 不把任何未派发的调用记为已开始 | 宪法第二条：没有发生的事不能记为发生 |
| S7 | 子 Run 的接纳结果未知 | Run 失败 | 结算为"未知" | 宪法第二条：未知是正当的终点，不应伪装为失败 |
| S8 | 有多条出边的调用节点 | 运行到该节点时 Run 失败 | 创建 Run 时拒绝 | 宿主代码无法替调用节点选择路由，这是作者错误，应尽早发现 |
| S9 | 旧版本写入的 `Deleted` 调用标记 | 恢复时按已完成处理 | H1 读取具名拒绝，需离线迁移 | 分支不背兼容包袱；旧数据迁移在合并时单独处理 |
| S6 | 并行区域已注定无法汇合（某分支触发上限） | 等本波次所有分支完成、并入结果后，下一波次才失败 | 立即结算为失败；之后到达的兄弟分支结果被拒绝，但连同产物证据完整记录在 Run 历史中 | 快速失败减少无效工作，与规格"分支失败时请求停止活动同伴"一致；被拒提交同样被记录，信息不丢失 |
| S10 | 任务历史中的成本 | 打回、拒绝、过期、待定等成本可能丢失 | 所有历史记录都计费，成本不推进版本；同一工作过期重提成本为 0 | D1，预算不能被拒绝提交绕过 |
| S11 | 残缺或损坏的历史 | 首个解析失败后静默停止，后续接受记录可能不可恢复 | 追加前去掉未闭合尾字节；闭合坏行具名报 Corrupt；完整历史字节不变 | D2，接受的事实必须可恢复，损坏不能静默吞掉 |
| S12 | Code 观察到未知效果 | Code stop 一律 HumanOnly | 有 Observation 可 stop 进入等待人；Code 无证据、Agent stop 与 Code resume 人的等待仍拒绝 | D3，安全方向与释放方向不对称 |
| S13 | 等待中的迟到节点结果 | Code/Human 可以继续写入并推进 Graph | 非生命周期提交统一 TaskWaiting 并计费；越预算仍结算失败，不应用节点内容 | D5 + D1，等待门禁由内核强制执行 |
| S14 | Graph 节点中断提交 | 无中断字段，无法保存宿主中断事实 | 仅 Code 可携带中断字段，记录规范中断结果并沿普通路由继续；非 Code 含显式 null 也拒绝 | D4，中断来自宿主观察，不能由 Agent 自述产生 |
| S15 | 旧 Run 格式、旧授权与旧 cursor | 加载时升级或派发前补写计数/授权 | H1 读取/接纳时拒绝并提示离线迁移，不改写数据；持久化记录仅接受对象 | C6 用户明确不兼容旧格式，拒绝隐式发明历史事实 |

S1、S2 在差分测试中被显式映射（S2：旧 `Stopped` 且存在停止原因 ⇔ 新 `Escalated`）。S1 未进入差分测试，因为路由脚本只产生合法路由。

## Host 接入预检：停止于内核边界

在 `d0f9b61` 上复现成本丢失、残缺尾行后接受记录不可恢复及 Code 无法进入等待人，按 C1 停止；原始观察与结构基线保留在[预检产物](../experiments/constitution-host-preflight/review.md)。审查随后将迟到 Code 结果从“不阻塞”改为 D5 必修；现由上述 K0.1 覆盖，旧建议不再作为实施依据。

## H1：旧格式清理（实现完成）

用户已决定 H1 仅做兼容删除；FileStore 随 H2 调度器接入，C2 零直接写入在 H3 结束验收。原[存储边界预检](../experiments/constitution-host-preflight/h1/review.md)保留为历史证据，不再是当前停止条件。

已删除格式自动迁移、managed-environment 遗留字段、旧授权升级、Deleted 调用标记及旧 cursor 计数补写。旧 Run 格式、遗留 environments、缺失授权摘要、Deleted 标记和旧 cursor 均具名拒绝，说明需离线迁移；读取不重写原始字节。当前持久化记录必须为对象，非对象序列也具名拒绝；解码保留 serde 已知字段重复键及未知字段拒绝，不经 Value 覆盖字段。未修改内核、GraphValidator 或三组差分测试；仍由旧 RunStore 单独负责 Run，无内核双写或新提交契约。

### 删除与替换的测试

以下旧 Runtime 测试名逐一退出；除授权测试改为执行前具名拒绝外，其余只为兼容/升级存在的测试删除，由统一旧格式拒绝覆盖：

- `a_format_ten_record_loads_without_inventing_plugin_views`。
- `a_managed_environment_record_migrates_by_dropping_the_field`。
- `a_rejected_legacy_plugin_does_not_partially_upgrade_other_plugins`。
- `deleted_graph_call_marker_resumes_as_completed_without_replay`。
- `format_one_cursor_migrates_under_lease_and_persists_once`。
- `format_one_terminal_run_migrates_without_changing_counters`。
- `format_seven_migrates_to_current_without_inventing_timestamps`。
- `format_twelve_migration_preserves_the_absence_of_plugin_authority_and_code_observations`。
- `legacy_plugin_authority_is_upgraded_only_when_the_original_code_is_still_installed`。
- `run_format_six_migrates_recovery_submissions_to_current_format`。
- `run_format_three_migrates_graph_calls_and_plugin_bindings_to_current_format`。
- `run_format_two_migrates_to_parallel_aware_format_three`。

Host 的 `legacy_mcp_preparation_is_read_only_and_runner_upgrade_requires_the_run_lease` 替换为 `legacy_mcp_preparation_and_load_require_offline_migration_without_rewrite`；`preliminary_wiring_accepts_only_unchanged_legacy_identity_and_validates_skill_authority` 替换为 `preliminary_wiring_refuses_legacy_identity_and_validates_skill_authority`。fake fixture 仅接受严格相等的现代 binding，不再借升级规则放行。

保留原业务断言：Run Plugin 历史顺序、停止 Run 的 Graph 修改/放弃、未完成 Run 阻止 Graph 删除、Plugin 更新影响这四处 fixture 补齐当前授权摘要；可变 MCP declaration 拒绝测试改用现代授权，以继续实际到达并验证原权限检查。没有删除这些业务/安全测试。新增旧格式 decode/FileRunStore 字节不变、旧 cursor/旧授权执行前拒绝、重复字段和非对象序列拒绝回归。

### H2/H3 直接写入棘轮起点

范围仅两个 Host crate 的生产 GraphRunRecord；按静态站点计数，排除测试、metadata、Session/Goose 自身记录。构造器及局部变量整体重绑定不算字段 mutation；集合 push/retain、mem::take 与 touch 单列。save 与 mutation 不可混为一个计数。下表为 H1 提交源码行号，后续只能减少，不能增加。

| Host | save | 字段赋值 | 集合修改 | touch | mutation 合计 |
|---|---:|---:|---:|---:|---:|
| anchor-runner-host | 15 | 22 | 4 | 7 | 33 |
| anchor-graph-host | 1 | 3 | 0 | 0 | 3 |
| **合计** | **16** | **25** | **4** | **7** | **36** |

### GraphRunRecord store.save（16）

| 文件:行号 | 写入 | 用途 |
|---|---|---|
| rust/anchor-runner-host/src/application/admission.rs:103 | `self.store().save(&record)?` | 新 Run admission；先持久化不可变身份元数据，再保存记录、注册并启动执行。 |
| rust/anchor-runner-host/src/application/assistants.rs:227 | `self.store().save(&record)?` | channel assistant 新 Run：元数据、执行准备及 workspace 绑定后保存。 |
| rust/anchor-runner-host/src/application/conversations.rs:855 | `self.store().save(&record)?` | conversation 新 Run：冻结附件/元数据及执行准备后保存，随后注册并启动。 |
| rust/anchor-runner-host/src/application/session_calls.rs:184 | `self.store().save(&record)?` | stop_session_call：非活跃且未终态记录保存为 Stopped。 |
| rust/anchor-runner-host/src/application/session_calls.rs:211 | `self.store().save(&record)?` | yield_session_call：非活跃且未终态记录保存为 Stopped。 |
| rust/anchor-runner-host/src/application/session_calls.rs:392 | `self.store().save(&record)?` | execute_session_call：保存中断工具恢复上下文、清空待恢复项并进入 Running，再启动。 |
| rust/anchor-runner-host/src/application/recovery.rs:139 | `self.store().save(&record)?` | 接受 Abort 恢复决定后保存 Aborted、错误与恢复事实。 |
| rust/anchor-runner-host/src/application/recovery.rs:145 | `self.store().save(&record)?` | 一项恢复决定完成但仍有未决项，保存 WaitingRecovery。 |
| rust/anchor-runner-host/src/application/recovery.rs:150 | `self.store().save(&record)?` | 最后一项恢复决定完成，保存 Running 后重新启动。 |
| rust/anchor-runner-host/src/application/control.rs:126 | `self.store().save(&record)?` | 停止 WaitingCall 父 Run，先保存 Stopped，再停止等待中的子 Run。 |
| rust/anchor-runner-host/src/application/control.rs:137 | `self.store().save(&current)?` | 停止子 Run 后重读父记录，若被推进回 WaitingCall，再保存 Stopped。 |
| rust/anchor-runner-host/src/application/control.rs:146 | `self.store().save(&record)?` | 普通 inactive stop：保存 Stopped，保留 cursor 与完成事实、不派发工作。 |
| rust/anchor-runner-host/src/application/abandon.rs:164 | `self.store().save(&record)?` | finalize_abandoned_run：非活跃且未终态 Run 保存为 Aborted 并记录放弃原因。 |
| rust/anchor-runner-host/src/execution.rs:317 | `self.store.save(&durable).map_err(...)?` | Runner 出错释放 lease 后，重新加载仍 Ready/Running 的记录，保存 host 中断失败。 |
| rust/anchor-runner-host/src/main.rs:663 | `execution.store.save(&record)`（`newly_admitted` 条件） | standalone CLI/协议入口，仅首次 admission 保存记录；已有匹配 snapshot/input 的记录不重存。 |
| rust/anchor-graph-host/src/graph_call.rs:532 | `self.store.save(record)` | persist_child_admission：catalog 子身份元数据/可选 Session call 元数据保存后，持久化 child Run。 |

### 字段赋值、集合修改与 touch（36）

| 文件:行号 | 写入 | 用途 |
|---|---|---|
| rust/anchor-runner-host/src/application/admission.rs:70 | `record.plugin_bindings = plugin_bindings` | 记录 admission 观察到的当前 Plugin bindings。 |
| rust/anchor-runner-host/src/application/admission.rs:71 | `record.plugin_bindings_initialized = true` | 标记 Plugin bindings 已初始化。 |
| rust/anchor-runner-host/src/application/admission.rs:102 | `record.touch()` | admission 保存前刷新记录时间。 |
| rust/anchor-runner-host/src/application/assistants.rs:184 | `record.plugin_bindings = plugin_bindings` | 新 assistant Run 记录 bundle Plugin bindings。 |
| rust/anchor-runner-host/src/application/assistants.rs:185 | `record.plugin_bindings_initialized = true` | 标记 assistant Run bindings 已初始化。 |
| rust/anchor-runner-host/src/application/conversations.rs:812 | `record.plugin_bindings = plugin_bindings` | 新 conversation Run 记录 bundle Plugin bindings。 |
| rust/anchor-runner-host/src/application/conversations.rs:813 | `record.plugin_bindings_initialized = true` | 标记 conversation Run bindings 已初始化。 |
| rust/anchor-runner-host/src/application/session_calls.rs:183 | `record.status = RunStatus::Stopped` | 停止未活跃、未终态 Session call。 |
| rust/anchor-runner-host/src/application/session_calls.rs:210 | `record.status = RunStatus::Stopped` | yield 非活跃、未终态 Session call。 |
| rust/anchor-runner-host/src/application/session_calls.rs:369 | `std::mem::take(&mut record.recovery)` | 清空并取走全部未决恢复项，逐项生成效果未知的恢复上下文。 |
| rust/anchor-runner-host/src/application/session_calls.rs:385 | `record.recovery_submissions.push(RecoverySubmission { ... })` | 追加中断工具调用的 Completed 恢复提交事实。 |
| rust/anchor-runner-host/src/application/session_calls.rs:391 | `record.status = RunStatus::Running` | Session call 恢复上下文记入后进入 Running。 |
| rust/anchor-runner-host/src/application/recovery.rs:123 | `record.recovery_submissions.push(RecoverySubmission { ... })` | Harness 意图持久化后追加本次恢复决定。 |
| rust/anchor-runner-host/src/application/recovery.rs:130 | `record.recovery.retain(...)` | 移除匹配 key + attempt_id 的已处理未决恢复项。 |
| rust/anchor-runner-host/src/application/recovery.rs:136 | `record.status = RunStatus::Aborted` | Abort 决定将整个 Run 置为终态；保留其他未决项作只读事实。 |
| rust/anchor-runner-host/src/application/recovery.rs:137 | `record.error = Some(format!(...))` | 写入 operator abort 的节点错误说明。 |
| rust/anchor-runner-host/src/application/recovery.rs:138 | `record.touch()` | Abort 分支保存前刷新记录时间。 |
| rust/anchor-runner-host/src/application/recovery.rs:143 | `record.status = RunStatus::WaitingRecovery` | 仍有未决恢复项，继续等待恢复。 |
| rust/anchor-runner-host/src/application/recovery.rs:144 | `record.touch()` | 继续等待恢复分支保存前刷新记录时间。 |
| rust/anchor-runner-host/src/application/recovery.rs:148 | `record.status = RunStatus::Running` | 未决恢复项全部处理后进入 Running。 |
| rust/anchor-runner-host/src/application/recovery.rs:149 | `record.touch()` | 恢复执行分支保存前刷新记录时间。 |
| rust/anchor-runner-host/src/application/control.rs:124 | `record.status = RunStatus::Stopped` | WaitingCall 父 Run 先停止，再处理等待的子 Run。 |
| rust/anchor-runner-host/src/application/control.rs:125 | `record.touch()` | 首次停止 WaitingCall 父 Run 保存前刷新时间。 |
| rust/anchor-runner-host/src/application/control.rs:135 | `current.status = RunStatus::Stopped` | 停止子 Run 后若父记录又为 WaitingCall，再次停止。 |
| rust/anchor-runner-host/src/application/control.rs:136 | `current.touch()` | 重读后的父 Run 保存前刷新时间。 |
| rust/anchor-runner-host/src/application/control.rs:144 | `record.status = RunStatus::Stopped` | 普通 inactive stop，保留执行 cursor 与完成事实。 |
| rust/anchor-runner-host/src/application/control.rs:145 | `record.touch()` | 普通 inactive stop 保存前刷新时间。 |
| rust/anchor-runner-host/src/application/abandon.rs:160 | `record.status = RunStatus::Aborted` | 非活跃、未终态 Run 由 operator abandon 置为 Aborted。 |
| rust/anchor-runner-host/src/application/abandon.rs:161 | `record.error = Some(format!(...))` | 写入 abandon reason 与 requested_at，保留放弃原因。 |
| rust/anchor-runner-host/src/execution.rs:313 | `durable.status = RunStatus::Failed` | 非 RunBusy/RunConflict 执行错误后，仅仍 Ready/Running 的持久记录置为 Failed。 |
| rust/anchor-runner-host/src/execution.rs:314 | `durable.error = Some(format!(...))` | 写入 host execution interrupted，声明未完成效果不会自动重放。 |
| rust/anchor-runner-host/src/main.rs:615 | `expected.plugin_bindings = plugin_bindings.clone()` | standalone 预期新记录绑定已解析 bindings；已有记录路径沿用加载记录。 |
| rust/anchor-runner-host/src/main.rs:616 | `expected.plugin_bindings_initialized = true` | 标记 standalone 预期新记录 bindings 已初始化。 |
| rust/anchor-graph-host/src/graph_call.rs:295 | `record.run_id = run_id.clone()` | child 构造后改用由 call identity 决定的可重试确定性 ID。 |
| rust/anchor-graph-host/src/graph_call.rs:296 | `record.plugin_bindings = referenced.iter().cloned().map(...).collect()` | child admission 记录当前 Library 解析的 Plugin bindings。 |
| rust/anchor-graph-host/src/graph_call.rs:301 | `record.plugin_bindings_initialized = true` | 标记 child Plugin bindings 已初始化。 |


RunStatus 直接赋值 11 站点已包括在 mutation 中；旧 Runtime Runner 的内部写入不在 Host 目录计数内，它随 H2 替换。基线只盘点写点，不证明并发恢复语义已迁移。验证命令、结果和未覆盖边界见[H1 验证证据](../experiments/constitution-host-preflight/h1/verification.md)。

## 尚未完成

- 并行分支内的模块作用域。
- Host 侧 H2–H6：H1 兼容删除已完成；后续接入调度器、执行者、内核 FileStore、生命周期及常驻助手中断契约。C2 在 H3 验收，16 save / 36 mutation 为上述棘轮起点。
- 旧 Runner 的删除。
- 测量探针与原因码投影接入。
