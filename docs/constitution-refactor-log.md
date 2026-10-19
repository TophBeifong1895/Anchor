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

按[审查决定 D1–D5](constitution-preflight-decision.md)完成限定范围实现；`88a6e41` 的[独立验收](../experiments/constitution-host-preflight/k01/review.md)九项通过，未发现阻塞问题。**当前状态：H1 已验收，下一步 H2**。K0.1 切片当时未修改 Host、旧 Runner 和现有内核测试断言。

- **D1**：选择从历史求和；两种 Store 的 `load` 共用成本投影，`spent` 为所有记录 `stamp.cost` 的饱和和，不再依赖版本化快照。打回、拒绝、版本过期、待定及已结算后的拒绝都计费，计费本身不推进版本。版本过期后以 `cost=0` 重提同一工作；超预算仍立即结算失败，结算推进版本。待定批准写入目标任务时的观察记录也遵守预算。
- **D2**：换行闭合才是完整记录；追加前截去尾部未闭合字节，保留完整记录原始字节。任何闭合坏行（包括末尾坏行、空行和无效 UTF-8）均报 `Corrupt`；末尾合法 JSON 若没有换行，也不算已提交记录。崩溃窗口仍从完整记录恢复，不重新执行。
- **D3 / D5**：Code 的 stop 必须有 Observation；人的等待只能由人 resume。等待中所有身份的非生命周期提交由内核拒绝并计费。等待中的非生命周期提交统一返回 `TaskWaiting`；若该次成本同时超预算，D1 仍将任务结算为预算失败，不应用节点内容。
- **D4**：可选 `NodeSubmission.interruption` 仅 Code 可携带，Agent 显式传 `null` 也拒绝。沿用既有规范中断 completion 与普通路由，写入 `RunResult.interruption`，保留现有并行区域边界；普通 Agent 提交序列化省略空中断字段。校验器不冻结或读取产物，`ArtifactKind::Interruption` 的执行者义务留给 H4；Host 的 suppressed 与投递账本未迁移。

原三个反例已成为内核正式回归，并保留隔离旧源码上的失败证据；另新增迟到 Code 结果、全部裁决计费、重试、损坏历史和中断校验测试。三组差分只补构造字段 `interruption: None`，断言和 20,000 例规模均未改。九项检查、实际命令和结果见[本轮验收产物](../experiments/constitution-host-preflight/k01/README.md)。

内核依赖、Record 格式、Waiter 与 Phase 变体均未增加。原预检目录保留为回归，原始失败 JSON 保持历史身份。**已知未决项**：崩溃后宿主重复提交的去重/重复计费本轮不处理；从历史求和的读取开销未做规模验收；固定 Goose、真实 Provider 与 Host 新调度路径不在 K0.1 验收范围。

## H1 补充：旧授权单一门

`ddadb17` 把空授权摘要从"每个使用点各自判断"改成"类型本身不可为空"：`PluginBinding.authorization_digest` 由 `String` 改为新类型 `AuthorizationDigest`（`rust/anchor-runtime/src/graph/ports.rs`），`TryFrom`/反序列化拒绝空值，错误信息为 `old authorization format; offline migration required`。方向已由架构师认可；该提交本身因下列测试问题被拒，由本节的修正提交补齐。生产代码里 `authorization_digest.is_empty()` 的判断点由 14 处降到 0 处。

### 14 处判断点的来源分类

按 `6ee2b0e`（上一版 H1 提交）逐一分类。`bindings` 来源指被检查的那一侧从哪来。"处置"列是这一刀的实际处理。

| # | 位置（6ee2b0e） | 检查对象与 bindings 来源 | 处置 | 理由 |
|---|---|---|---|---|
| 1 | `anchor-runner-host/src/execution.rs:156` | 调用方传入的 bindings：已解码持久化 Run 的 `plugin_bindings`（应用路径）或 catalog 当前定义（CLI 路径） | 删除 | 已解码的持久化 Run 不可能带空值；catalog 定义由本刀改为 `AuthorizationDigest`，同样不可为空 |
| 2 | `execution.rs:170` | `resolve_plugins` 返回的当前定义 | 删除 | 当前定义的授权摘要由 `FilePluginCatalog` 计算，新类型保证非空 |
| 3 | `execution.rs:182` | admitted 侧：已解码持久化 Run | 删除 | 空值已不可能；该处原本用于在老授权与"授权已变"之间选择错误信息，现在只剩后者 |
| 4 | `tool_host.rs:144` | `from_catalog` 入参：已解码持久化 Run 或 catalog 当前定义 | 删除 | 同一类型保证；反序列化已是唯一拒绝点 |
| 5 | `tool_host.rs:193` | 同上，逐 binding 再查一次 | 删除 | 与 #4 同一规则的第二处判断，纯冗余 |
| 6 | `anchor-runtime/src/graph/runner.rs:854` | `record.plugin_bindings`：已解码持久化 Run | 删除 | 已解码记录不可能带空值 |
| 7 | `runner.rs:878` | `resolve_plugins` 返回的当前定义 | 删除 | 当前定义的授权摘要由 catalog 计算，非空 |
| 8 | `runner.rs:914` | admitted 侧：已解码持久化 Run | 删除 | 同 #3，错误信息分支合并为一条 |
| 9 | `anchor-runtime/src/graph/state.rs:352` | 解码入口：持久化字节 | 删除（由新类型接管） | 这一处才是真正的门。删除判断，改由 `PluginBinding::deserialize` 内的 `AuthorizationDigest::try_from` 拒绝，语义等价且无法被绕过 |
| 10 | `state.rs:473` | `GraphRunRecord::validate`：内存记录 | 删除 | 内存记录只由解码或构造产生，两条路都不可为空 |
| 11 | `ports.rs:186` | `same_authorization` 的 `self`：已解码持久化 Run | 删除 | 空值不可构造；保留比较即可 |
| 12 | `anchor-runner-host/src/assistant_plugins.rs:143` | 助手 bundle 的 publication 回执 | 删除 | 回执经反序列化读出，空值已被拒绝；`id != binding.id` 的结构检查保留 |
| 13 | `assistant_plugins.rs:214` | 助手 bundle 的 Turn policy | 删除 | 同上 |
| 14 | `ports.rs:177` | 归因：`skip_serializing_if = "String::is_empty"` 使空值在序列化时消失 | 删除并改为结构化 | 这正是"空值"这种旧状态得以写出的原因。新类型 `Serialize` 恒输出字段，"缺失"不再被写入，只能在读到旧文件时出现 |

计数：**旧 14 处（生产代码）→ 新 0 处**，拒绝只在反序列化一层产生。测试代码里还有 3 处 `is_empty()`/`clear()`（`legacy_plugin_tests.rs:104`、`legacy_plugin_tests.rs:119`、`preparation_tests.rs:299`），其中 2 处是构造旧格式的夹具，随本节改动一并调整；`legacy_plugin_tests.rs:104` 的断言保留（它验证的是 catalog 真的算出了摘要）。

`assistant_plugins::active_bindings` 随同收窄为 `active_plugin_ids`：旧版本在缺策略时会用 `PluginBinding { authorization_digest: String::new(), .. }` 现场合成一个绑定，这正是空授权在内存里唯一的来源；改为只返回 id 集合后，这条合成路径在类型上消失。唯一调用点 `application/plugins.rs:763` 只做 `contains_key`，改为 `contains` 后语义不变。

### 改写与恢复的测试

| 测试 | 原覆盖 | 本刀前（ddadb17） | 本刀处理 |
|---|---|---|---|
| `anchor-runtime/src/graph/tests.rs: legacy_plugin_authority_requires_offline_migration_before_execution` | runner 行为：带空授权的记录不派发节点、store 无记录、拒绝信息具名 | 退化为纯解码测试（只断言反序列化报错） | **恢复 runner 行为覆盖**：解码仍逐个断言"缺失/null/空"三种旧形状；另用 `FileRunStore` 写入旧字节后 `GraphRunner::run`，断言返回具名拒绝、`nodes.calls` 为空、文件字节不变 |
| 同上文件 `a_new_run_does_not_bind_plugins_without_a_current_authorization_baseline` | runner 行为：resolver 给出不可用基线时新 Run 失败、不绑定、cursor 与 invocations 为空、节点未被调用 | 退化为纯新类型测试 | **恢复 runner 行为覆盖**；原夹具用的空授权在新类型下不可构造，改用 `digest` 为空（resolver 返回未哈希绑定）触发同一条失败路径，原断言与错误文本 `unauthorized` 全部保留。新类型的 `try_from`/序列化断言保留在测试尾部 |
| `anchor-runner-host/src/execution/legacy_plugin_tests.rs: legacy_mcp_preparation_and_load_require_offline_migration_without_rewrite` | prepare 与 `store.load` 都具名拒绝旧授权，且文件字节不变 | 夹具同时把 `format` 改成 12 并把授权摘写成空串 | **拆成两个用例**：`(无 format 改写, 期望 old authorization format)` 与 `(format=12, 期望 old Graph Run format)`，两者都断言 prepare 拒绝、load 拒绝、字节不变；并新增 `assert_no_execution_state` 检查拒绝后没有产生任何带 run_id 的执行状态 |
| 同文件 `changed_or_ambiguous_legacy_mcp_bindings_fail_before_durable_state_changes` | 四种变更（code/resources/servers/plugin_id）下，prepare 拒绝且持久化字节不变 | 保留了"传入当前 bindings"的分支，但那条分支在新代码里报的是"授权已变" | 保留两个真实路径并各自断言正确的拒绝：应用路径只能先 `store.load`（断言 `old authorization format`）；CLI 路径传入当前 bindings 时断言 prepare 抵达持久化解码并同样拒绝旧授权。原意图（拒绝 + 字节不变 + 无执行状态）保留 |
| 同文件 `a_waiting_parents_preparation_validates_its_legacy_child_without_upgrading_either` | 等待中子 Run 的旧身份被拒绝，父子字节均不变 | 断言仅"含 offline migration required" | 断言收紧为同时含 `old authorization format`，明确拒绝来自子 Run 的授权解码（父记录本身不含 binding） |
| `anchor-runner-host/src/tool_host/preparation_tests.rs: preliminary_wiring_refuses_legacy_identity_and_validates_skill_authority` | 旧身份装配被拒；**代码改动后同一 binding 失去授权** | 末段退化为重复一次反序列化断言，丢失"装配期拒绝已变更定义"的覆盖 | **恢复装配期覆盖**：保留反序列化拒绝断言；另在 `plugin.json` 新增一个 MCP server（真正改变授权摘要的输入）后断言 `from_catalog` 拒绝旧 binding。原测试改的是 `instructions.md`，而那并不进入授权摘要，旧断言此前是由空授权短路才通过的 |
| `anchor-runner-host/src/tool_host.rs: fake_fixture_binding_does_not_require_a_filesystem_catalog` | 旧授权装配被拒 | 改为断言反序列化报错 | 保留（装配侧已不可构造空授权，类型保证替代；装配期拒绝的覆盖由上一行恢复） |
| `anchor-runtime/src/graph/tests.rs: old_run_formats_are_named_refusals_on_decode_and_file_load` | 各旧格式逐个具名拒绝 | 断言放宽为"三种旧格式之一" | 保留（夹具与期望不是一一对应的放宽仍存在，属 H1 原有风格；本刀未再放宽。待办见下） |
| `anchor-runtime/src/graph/tests.rs: newer_run_formats_are_refused_instead_of_guessed` | 新格式被拒 | 由"decode 通过 + validate 拒绝"改为"decode 拒绝" | 保留（`RunRecordFormat` 新类型使不合法格式无法构造，validate 分支随之不可达） |

未新增测试函数，只强化既有断言；`anchor-runtime` 的 `AuthorizationDigest` 两个 `compile_fail` doctest 由 ddadb17 引入，本刀验证其确实失败编译（`cargo test -p anchor-runtime --doc`：2 通过）。注意 `--all-targets` 不运行 doctest，因此它们不计入全量测试数字。

### 第 1、2 项的定性

- **第 1 项（两个确定性失败的 Host 夹具测试）**：定性为**夹具过时**，不是生产路径丢失拒绝。ddadb17 使空授权摘要不可构造，而两个夹具仍假定"内存里可以持有一条空授权 binding"：`legacy_record` 在内存中清空授权摘要，`legacy_file` 把 `format` 改成 12。在新类型下 prepare 能得到的历史数据只有解码后的记录，所以拒绝要么发生在 `store.load`（旧格式），要么在"传入当前 bindings"的路径上以授权比对失败的形式出现。裁定：保留原意图改写夹具（旧数据被拒绝 + 持久化字节不变 + 未执行节点），并把 runner/装配层的拒绝覆盖补回来，见上表。生产代码只保留解码层这一道门，没有丢拒绝；丢掉的只是"逐 Plugin 指名的错误文本"，登记为 S16。
- **第 2 项（两个满载下失败的测试）**：在 `445d278` 的 6 次全量运行中未复现，但同一故障类别（`Goose conversation is already active`、进程内 env 竞态）在基线上复现了 3 次。定性为**既有 flaky，不在本切片修**，证据见"待处理与观察"一节。

### 语义变更

本刀不改变对外行为：旧数据的拒绝结论、"读取不改写"、拒绝的粒度（整条记录）都保持一致，只是拒绝点从 14 处收敛到解码一层。唯一可观察的差异是错误信息的保真度，登记为 S16。

`ports.rs` 的 `skip_serializing_if` 删除后，新写出的记录字节不变：该属性只对空值生效，而新类型不可能为空，非空摘要原本也照常写出。

## 待处理与观察：全量运行中的既有 flaky

为定性 `ddadb17` 之后全量运行里两个"满载才失败"的测试，在临时 worktree `/tmp/h1-base`（`445d278`，`git worktree add`，用后已 `git worktree remove`）用同一条全量命令跑了 6 次：

| 运行 | 结果 | 失败项 |
|---|---|---|
| 1 | 1277 通过 / 1 失败 / 112 忽略，45 targets | `runtime_management::tests::runtime_python_version_is_a_refreshable_observation_without_publishing_configuration` |
| 2 | 1278 / 0 / 112 | — |
| 3 | 1276 / 2 / 112 | `goose_acp::conversation::tests::latest_native_fact_may_be_an_authorized_background_continuation_not_chain_head`、`run_data::tests::conversation_cleanup_keeps_other_scopes_and_obeys_native_lock`（均为 `Goose conversation is already active` / `native conversation ... is already active`） |
| 4 | 1278 / 0 / 112 | — |
| 5 | 1277 / 1 / 112 | `goose_acp::conversation::tests::latest_native_fact_may_be_an_authorized_background_continuation_not_chain_head`（`Goose conversation is already active`） |
| 6 | 1278 / 0 / 112 | — |

结论：

1. **两个报告项在 `445d278` 上 6 次运行均未复现**（`api::tests::session_plugins::only_the_configured_pilot_graph_may_declare_a_plugin_required`、`goose_acp::conversation::tests::reviewed_provider_patches_load_old_history_only_for_a_new_invocation` 都是通过）。所以"由 `ddadb17` 引入"没有被证实，但也没有被排除。
2. **故障类别在 `445d278` 上确实存在且可复现**：6 次里有 2 次（第 3、5 次）出现 `Goose conversation is already active`，落在同一个 `goose_acp/conversation` 测试文件的两个兄弟测试上；另有 1 次（第 1 次）是进程内环境变量竞态导致的运行时探测失败。三者都不是本刀触碰的代码路径。
3. 因此判定为**既有 flaky，不在本切片修**，登记在此：这一类测试在同一二进制内共享进程级 conversation lease / Library 读锁 / `PROCESS_ENV_LOCK`，满载时相互竞争；`runtime_python_version` 是同一进程内 `std::env` 竞态。代表性证据：第 3 次的失败信息与报告项的一字不差。
4. 未做的验证：没有为这些测试做串行化改造或隔离，也没有扩大采样规模。若要归零，需要单独一刀（例如给共享 lease 的测试加进程级串行门），属于 H2 之前的独立工作。

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
| S16 | 空授权摘要的拒绝信息 | `Plugin \`{id}\` uses an old authorization format; offline migration required`，逐 Plugin 指明 id | `old authorization format; offline migration required`，由解码层给出整条记录的拒绝，不含 Plugin id（serde 另附行列位置） | 判断点由 14 处收敛到 `AuthorizationDigest` 反序列化一层，解码层已不持有"是哪个 Plugin"的语境；调用方只依赖 `offline migration required` 这一子串，无分支受影响 |
| S17（已批准 2026-10-17） | 宿主确认节点失败 | 计入开始次数，无结果、无出边决定；并行同波多失败时逐个覆盖 Run error，最终错误取决于波内处理顺序（`parallel.rs:278`） | Code 以 Settle 结算，保留计数和分支失败事实；首次被接受的失败决定唯一结算原因，后续失败以 TaskSettled 被拒并留在历史中；只有首个失败分支为 Failed | 旧"最后一个获胜"是波次处理顺序的偶然，新调度器无波次屏障，无从复刻；与 S6 fail-fast 一致；结算不可逆；其他失败事实不丢失。对照规则见 H2 任务书 §1 测试第 3 条 |
| S18（已批准 2026-10-17，合并 S6、S17） | 并行区内多个终结事实（节点失败、分支轮次上限）在同一波出现，例如 seed 2122：单个节点失败 + 成功兄弟的后继已达上限 | 波次屏障：收齐整波后处理，最终原因取决于波内顺序；本例为 b1_0 failed，ceased 为空 | 第一个被内核接受的终结事实决定唯一结算原因；之后的终结事实与结果以 TaskSettled 被拒并留在历史中；本例为 b0_1@1 上限 | 新模型无波次屏障，结算原因由提交顺序决定，是 S6 fail-fast 与 S17 的同一规则。对照改为 H2 任务书 §1 的三条不变量（原因正当、事实不丢不重、其余相等），seed 495/2122 用例的断言替换由审查方批准 |

S1、S2 在差分测试中被显式映射（S2：旧 `Stopped` 且存在停止原因 ⇔ 新 `Escalated`）。S1 未进入差分测试，因为路由脚本只产生合法路由。

## Host 接入预检：停止于内核边界

在 `d0f9b61` 上复现成本丢失、残缺尾行后接受记录不可恢复及 Code 无法进入等待人，按 C1 停止；原始观察与结构基线保留在[预检产物](../experiments/constitution-host-preflight/review.md)。审查随后将迟到 Code 结果从“不阻塞”改为 D5 必修；现由上述 K0.1 覆盖，旧建议不再作为实施依据。

## H1：旧格式清理（实现完成）

H1 已验收，下一步 H2。`445d278` 之后用户独立重跑确认 45 targets、1278 通过、0 失败、112 忽略，Clippy 与 fmt 通过。`ddadb17` 的旧授权单一门方向已获认可，但其测试问题被验收拒绝，由 `H1 补充：旧授权单一门` 一节的修正提交补齐（夹具过时、覆盖恢复见该节）。用户已决定 H1 仅做兼容删除；FileStore 随 H2 调度器接入，C2 零直接写入在 H3 结束验收。原[存储边界预检](../experiments/constitution-host-preflight/h1/review.md)保留为历史证据，不再是当前停止条件。

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

### H1 之后修复：并发 fork 导致的锁释放延迟

**根因**  
Linux flock 特性：fork 后父子进程同时持有锁文件 fd 副本，直到所有副本关闭，锁才真正释放。`goose_acp/conversation.rs:63` (flock) 和 `run_data.rs:119` (try_lock) 两处只依赖 drop 隐式关闭 fd，未显式 unlock。当 Host 高频 spawn 子进程（runtime provision、插件探测、artifact 处理）时，子进程在 exec 完成前会继承父进程的锁 fd，导致父进程 drop 后锁仍未释放。下一个请求立刻 try_lock 拿不到，误报 "conversation is already active" 返回 409。

对比实验验证：只 drop 不 unlock 时，单 spawner 20000 次中假阳性 2255 次，8 spawner 10353 次；显式 unlock 后 drop，单/8 spawner 各 20000 次假阳性均为 0。

**修复**  
- `rust/anchor-runner-host/src/goose_acp/conversation.rs:33-39`：为 `ConversationScope` 添加 Drop impl，在 drop 时显式 `flock(..., Unlock)`，参照 `graph/store.rs:97` 的 `FileRunLease::drop`。
- `rust/anchor-runner-host/src/run_data.rs:177-180`：`remove_legacy_conversation` 函数在最终 return 前显式 `lock.unlock()`。

**测试修复**  
`runtime_management.rs` 测试中 `configure_python` 辅助函数原直接 `fs::write` 写入 python 桩脚本，该 fd 会被并发测试线程 fork 出的子进程继承，导致 exec 这个脚本时触发 ETXTBSY (Text file busy)。修改为通过子进程完成写入（`sh -c 'cat > path'`），写 fd 仅存在于子进程内，不会被测试进程的其他线程 fork 继承。同时修复测试内 307、349 行同样会被 exec 的脚本覆写操作。

独立实验证实：直接 write + exec，3000 次有 56 次 ETXTBSY（单 spawner），8 spawner 时 634 次；改用子进程写入后 0 次。

**影响范围**  
生产缺陷，影响真实并发场景（多用户 Goose 会话、多 Run cleanup）。修复同时消除测试 flaky。

`ddadb17` 之后这两个替换测试的 runner/装配层覆盖曾退化为纯解码断言，已在 `H1 补充：旧授权单一门` 一节恢复；那一节另有逐测试的覆盖去向表。

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

## H2.1：本地确定性验收完成

按已批准 S18 统一失败对照：首次终结事实决定原因；每个失败与 Work 结果在状态或迟到历史中恰好出现一次；还原允许的迟到与上限效果后，其余事实相等。seed 495、2122 独立用例通过，生产校验器、内核及旧 Runner 未改。

串行 10000（node_failed=772）、并行失败组 5000（node_failed=775、multi_failure_waves=6、ceiling_preempted_failures=2）、调用 5000 均通过。用户“继续”后采用交回建议，原无失败回归使用同一组 5000 个 seed 单独保留原门槛：parallel_activations=5048；失败组仍为4775，不伪装成超过5000。失败哈希、注入概率、seed范围、原阈值均不变。

Runtime 127通过，另2 doctest通过；fmt、Clippy通过，内核diff为空。Kernel与Host未改，复用本轮此前37与1120通过/112忽略结果。真实Provider和Goose未新增验收。详细Tally及原始日志见[H2.1验收](../experiments/constitution-host-preflight/h2/h21-completion.md)。后续改为新版任务书的串行探针P；原H2.2/H2.3不再推进。

## P：串行 Kernel 探针本地交付完成

实现提交 `ff09f2c feat(host): kernel-path probe for serial Runs (P)`。同一Host准入显式选择 `execution=kernel_probe`，Run只由Kernel FileStore保存，调度器复用现有请求授权与NodeExecutionPort/ArtifactPort；默认旧Runner保留。Human stop/resume/abandon经Kernel裁决，接受后再取消worker；未知效果带Observation等待人核查，迟到结果进入拒绝历史，不自动重放。状态是Phase的只读投影，同ID双权威拒绝；未改Kernel与生产Graph裁决。

七类应用层场景、1000个共享生成器seed对照及真实Op.run/Bubblewrap/Artifact路径通过。Kernel37、Runtime127另2 doctest、runner-host1083/0失败/112忽略、graph-host46/0失败；fmt、Clippy与sandbox构建通过。真实Goose/Provider未执行，固定二进制未配置。证据及全部最终命令见[P验收](../experiments/constitution-host-preflight/p/verification.md)。

测量去重包络：旧193个语法判断点/1416源码行，新51个/472行（新增449行）；旧包络含未迁能力，不能把差值当同等能力净收益，实际删除旧生命周期实现0行。全Host粗估与口径、逐点源码位置、Unsupported和查询适配耦合见[测量与未决](../experiments/constitution-host-preflight/p/measurement.md)。fanout/join、Call、常驻助手、pause、真实Goose attempt恢复与Kernel Run删除未迁；P任务已交付，后续迁移范围按审查结论再定。

## P 与 H2.1 审查结论（2026-10-17）

审查方独立重跑四 crate：48 个 target，1295 通过 / 0 失败 / 112 忽略。内核相对 `6ee2b0e` 零改动；调度器和 `kernel_runs.rs` 中无停止/暂停轮询；差分阈值与 seed 范围未降，并行组 `parallel_activations > 5_000` 门槛移到无失败对照样本上，属于保留而非弱化。H2.1 与 P 验收通过。

结论：写入与生命周期一侧成立（stop 40→7、resume 38→10 个判断点，旧 8 处停止/暂停轮询在新路径为 0）；跨能力的总量对比（193→51）不同口径，不作为收益证据，实际删除旧代码为 0。读取一侧未证：P 把 Phase 投影回 RunStatus，`queries.rs` 复用 FileStore 文件名编码取时间。下一步为限时 2 天的读模型探针 R，见 [R 任务书](constitution-r-task.md)；其中 R0 将 `469d81f` 的锁释放修复收口为唯一文件租约类型（`run_data.rs` 提前返回路径仍未显式解锁，生产代码另有约 15 处同类 flock）。

## R0 / R：锁收口与统一读模型探针本地交付

R0 `cb2219c` 提供唯一 FileLease，在Drop显式释放，覆盖提前返回与fork继承描述符；22个底层获取站点和6个显式释放站点收口为共享实现的1获取/1释放，22个调用者继续保留原锁策略。已同步master `6394e28`，保留master原有共享读锁和可见性；master相关check与132项定向测试通过。

R `54e664e` 引入只读RunView（Phase、Graph passes、worker、宿主updated），按相位派生动作；Session条件继续由调用者处理。接受提交后记录Host观察文件，不读取Kernel路径或mtime；观察失败不改变控制。旧Runner适配和原生JSON兼容边界单列，旧Failed的缺省原因用Unclassified，不虚构原因码，也不回写兼容记录。

结论为**需要重想**：同口径I/M/W判断点82→90。四个消费者生产RunStatus引用清零，但旧Ready/Running、Paused/Stopped、预算/recovery与P孤立driver启动等规则没有消失，不能全归属Session/授权。旧适配仍读取原record，严格的单RunView输入未闭合，不记为统一迁移完成。H3–H6继续暂缓，待审查旧显示/动作契约。测量、逐项保留规则和剩余18个原文件估算见[R测量](../experiments/constitution-host-preflight/r/measurement.md)。

最终Kernel37、Runtime127+2 doctest、runner-host1087/112原有忽略、graph-host46、Library74、FileLease3通过，0失败；比四crate基线新增4项，另R0新增3项锁测试。fmt、Clippy和sandbox构建通过；Kernel相对6ee2b0e、生产GraphValidator相对P均零diff。真实Goose/Provider、Web与部署未新增验收。原始命令、输出、HTTP/动作/字节兼容测试及未覆盖边界见[R验证](../experiments/constitution-host-preflight/r/verification.md)。

## R 审查结论与 R2 决定（2026-10-17）

审查方重跑六 crate（含 file-lease、library）：58 个 target，1376 通过 / 0 失败 / 112 忽略；内核、生产校验器、Web diff 为空。R0 通过：22 个调用站点统一经共享租约，Drop 显式释放；已同步 master 本地提交 `6394e28`（未推送）。R 的负结论（82→90）成立，但前提是任务书要求对外 JSON 不变：旧 10 状态中 Ready/Running/waiting_resume、Paused/Stopped/BudgetStopped 的区分不在相位里，规则只能搬进适配。读侧复杂度来自产品状态契约，不来自运行时。

**用户决定**：对外状态收敛到内核相位（executing / waiting(human|child|effect) / settled(outcome) + reason + active + actions），允许改 API 与 Web，pause 改为 stop 的别名。R2 在新词汇下以同一口径重测，见 [R2 任务书](constitution-r2-task.md)。


## R2停止交回（2026-10-19）

源码 `74a4dec`：Run 对外收敛为 executing / waiting / settled + waiter/outcome/reason/active/actions/updated；旧状态只经一个有损映射函数进入读视图，HTTP/Pilot pause 接为 stop 别名，Web 使用新事实。**R2 已停止、未验收**：pause 即时取消后，原“当前节点完成并留下 1 条结果”的非状态断言实得 0；Session 原 pause 拒绝也变为有效 stop。未修改实际 stop/resume/abandon 规则、未改这些非状态断言，按任务书 §7 交回。审查需决定是否授权调整原暂停测试场景，或保留对外暂停能力；不以探针提交代表验收通过。

同口径测量 **82→90→92**，Web 状态分支 **81→79**，需要重想；H3–H6 继续暂缓。保留 Session 写回和内部 record 桥均计回，没有通过搬移制造收益。当前证据：[测量](../experiments/constitution-host-preflight/r2/measurement.md)、[92 点逐项](../experiments/constitution-host-preflight/r2/decisions.md)、[实际验证与停止现场](../experiments/constitution-host-preflight/r2/verification.md)。

Rust API 定向 188 过/11 失败，最终针对 11 例复跑仍失败；映射/动作/静止 3 例、投影 4 例、pending 1 例通过。application 最终子集 113 过/1 候选进程回收时序失败，该例独立复跑通过；1000 seed 已在初轮通过。fmt/clippy/sandbox 构建通过，Kernel 零 diff。六 crate 全量未执行；Web 33 文件/285 测试与 build 通过，定向 e2e 9 过/1 选择器失败，修复后该例独立通过。真实 Goose/Provider、部署、同一 HTTP 的旧/Kernel 三阶段对照仍未验收。

S 候选 8 条，全部待批：

| 编号 | 旧行为 → 新行为 / 影响 |
| --- | --- |
| S19 | 旧十状态/专门提示 → Phase+waiter/outcome/reason，客户端字段破坏；Ready/孤立 Running 同为等待接续和注意项 |
| S20 | 共享 RunView 的无 worker Executing 可以 resume；原 Kernel JSON 已有独立 driver 接续规则，现统一 |
| S21 | Paused/BudgetStopped 无 worker 统一静止，影响 Session 交接、准入、退役与清理 |
| S22 | stop 仅 Executing 为 true、不读 pending flag，未结算增加 abandon；控件按服务端事实显示 |
| S23 | Human 无 worker 可 resume、Child/Effect 不可；旧预算/recovery 写守卫仍在，可能与展示不一致 |
| S24 | 移除 pausing/stopping/control_requested 与请求优先级展示，接受请求但未落状态时仍显示 Executing |
| S25 | pause→stop 即时取消，影响当前节点结果、恢复、Session 取消许可；已触发停止条件 |
| S26 | 缺少 Kernel 时间观察返回 updated:null，旧 Run mtime 来源保留，时间不参与控制 |

自行判断并登记：Session 原结算 callback 原样搬回 conversations 属于写边界；Phase 序列化和旧内部桥都保留测量成本；PilotTurn/listener/投递/节点结果保持各自事实；不新增旧状态替身。未迁能力没有记作已完成。

### R2 断言修改附录

以下逐项列出测试名、旧/新断言及 S 候选。Rust **84 条展开断言修改 + 6 条轮询目标**，另加别名检查 2 条；Web **30 处直接表达式修改**，参数实例/fixture 间接变化单列。两侧统计单位不同，不相加冒充 assert 宏数量。原非状态结果、返回码、权限、时间和产物断言均保留；pending token 被删除后的替换不再检验原展示优先级，已明确登记 S24。完整机读附表：[Rust](../experiments/constitution-host-preflight/r2/rust-assertions.json)；审计副本：[Rust](../experiments/constitution-host-preflight/r2/rust-assertions.md)、[Web](../experiments/constitution-host-preflight/r2/web-assertions.md)。

#### 逐项既有断言、轮询与新增别名检查

##### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`graph_crud_round_trips_authoring_modules_and_layout`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA001 / 修改轮询 | 等待执行结束的轮询条件 | `HTTP OK && state.status != running && state.status != ready` | `HTTP OK && projection.phase != executing` | S19：新 phase/outcome 词汇 |
| RA002 / 修改断言 | 完成结局 | `detail[state][status] == completed` | `detail[projection][outcome] == done` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`trigger_persists_run_and_projects_list_detail_and_control`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA003 / 修改轮询 | 等待执行结束的轮询条件 | `HTTP OK && state.status != running && state.status != ready` | `HTTP OK && projection.phase != executing` | S19：新 phase/outcome 词汇 |
| RA004 / 修改断言 | 完成结局 | `detail[state][status] == completed` | `detail[projection][outcome] == done` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`paused_wait_child_resumes_independently_then_releases_parent`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA005 / 修改轮询 | 子 Run 停下的轮询条件 | `store child.status == RunStatus::Paused` | `store child.status == RunStatus::Stopped` | S25：pause 别名即时 stop |
| RA006 / 修改断言 | 子 Run 的存储状态 | `store child.status == RunStatus::Paused` | `store child.status == RunStatus::Stopped` | S25：pause 别名即时 stop |

##### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`start_and_pause_request (fixture)`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA007 / 修改断言 | fixture 最终存储状态 | `record.status == RunStatus::Paused` | `record.status == RunStatus::Stopped` | S25：pause 别名即时 stop |
| RA008 / 新增断言 | 新增：别名的有效操作 | `无该断言` | `alias[operation] == stop` | S25：pause 别名即时 stop |
| RA009 / 新增断言 | 新增：响应说明别名 | `无该断言` | `alias[alias] == true` | S25：pause 别名即时 stop |

##### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`downstream_failure_does_not_reclassify_committed_upstream_result`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA010 / 修改断言 | 失败结局 | `detail[state][status] == failed` | `detail[projection][outcome] == failed` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`waiting_parent_rejects_child_grant_drift_then_restores_same_child_without_replay`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA011 / 修改断言 | 停下后的子 Run 存储状态 | `child_before.status == RunStatus::Paused` | `child_before.status == RunStatus::Stopped` | S25：pause 别名即时 stop |

##### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`conversation_resume_is_not_blocked_by_another_session_on_the_same_graph`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA012 / 修改轮询 | bob_run 的 wait_run_status 目标 | `RunStatus::Paused` | `RunStatus::Stopped` | S25：pause 别名即时 stop |

##### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`superseded_conversation_cannot_resume_or_recover_its_wait_child`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA013 / 修改轮询 | child_run 的 wait_run_status 目标 | `RunStatus::Paused` | `RunStatus::Stopped` | S25：pause 别名即时 stop |

##### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`superseded_conversation_keeps_detached_child_independent`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA014 / 修改轮询 | detached child_run 的 wait_run_status 目标 | `RunStatus::Paused` | `RunStatus::Stopped` | S25：pause 别名即时 stop |

##### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`superseded_conversation_cannot_resume_or_recover_its_wait_child`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA015 / 修改断言 | 子 Run 对外相位 | `detail[state][status] == stopped` | `detail[projection][phase] == waiting` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`conversation_orphan_running_requires_explicit_stop_before_a_new_turn`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA016 / 修改断言 | 无 worker 的 Running 对外相位 | `detail[state][status] == running` | `detail[projection][phase] == executing` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/kernel_probe.rs`

测试 / fixture：`explicit_kernel_probe_runs_real_host_ops_and_projects_existing_queries`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA017 / 修改断言 | 详情相位 | `detail[state][status] == completed` | `detail[projection][phase] == settled` | S19：新 phase/outcome 词汇 |
| RA018 / 修改断言 | 详情结局 | `detail[projection][terminal] == true` | `detail[projection][outcome] == done` | S19：新 phase/outcome 词汇 |
| RA019 / 修改断言 | 列表结局 | `list[runs][0][status] == completed` | `list[runs][0][projection][outcome] == done` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/kernel_probe.rs`

测试 / fixture：`kernel_read_view_http_tracks_running_human_wait_and_settlement_with_host_time`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA020 / 修改断言 | 运行中相位 | `running[projection][phase] == running` | `running[projection][phase] == executing` | S19：新 phase/outcome 词汇 |
| RA021 / 修改断言 | 运行中动作精确 JSON | `{pause:false,stop:true,resume:false}` | `{stop:true,resume:false,abandon:true}` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA022 / 修改断言 | 等待人相位 | `waiting[projection][phase] == stopped` | `waiting[projection][phase] == waiting` | S19：新 phase/outcome 词汇 |
| RA023 / 修改断言 | 等待人仍有 worker 的动作精确 JSON | `{pause:false,stop:false,resume:false}` | `{stop:false,resume:false,abandon:true}` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA024 / 修改断言 | 已放弃相位 | `settled[projection][phase] == aborted` | `settled[projection][phase] == settled` | S19：新 phase/outcome 词汇 |
| RA025 / 修改断言 | 已放弃结局 | `settled[projection][terminal] == true` | `settled[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |
| RA026 / 修改断言 | 已结算动作精确 JSON | `{pause:false,stop:false,resume:false}` | `{stop:false,resume:false,abandon:false}` | S22/S24：动作按相位推导、移除 pending-control 展示 |

##### `rust/anchor-runner-host/src/api/tests/platform_contract.rs`

测试 / fixture：`trigger_freezes_objective_and_manual_or_schedule_source`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA027 / 修改断言 | 去掉旧 pending-control 字段后的状态断言 | `detail[control_requested] == null` | `detail[projection][phase] == settled` | S22/S24：动作按相位推导、移除 pending-control 展示 |

##### `rust/anchor-runner-host/src/api/tests/platform_contract.rs`

测试 / fixture：`updated_tracks_persisted_pause_stop_and_resume_after_restart`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA028 / 修改断言 | pause 别名后的相位 | `paused[state][status] == paused` | `paused[projection][phase] == waiting` | S19：新 phase/outcome 词汇；S25：pause 别名即时 stop |
| RA029 / 修改断言 | 重启后状态（原查 pending-control） | `reopened[control_requested] == null` | `reopened[projection][phase] == waiting` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA030 / 修改断言 | stop 后相位 | `stopped[state][status] == stopped` | `stopped[projection][phase] == waiting` | S19：新 phase/outcome 词汇 |
| RA031 / 修改断言 | resume 完成后相位 | `completed[state][status] == completed` | `completed[projection][phase] == settled` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/run_abandon.rs`

测试 / fixture：`abandon_cancels_the_running_node_and_makes_the_run_terminal_aborted`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA032 / 修改断言 | abandon 响应结局 | `body[status] == aborted` | `body[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/run_abandon.rs`

测试 / fixture：`abandon_is_idempotent_and_rejects_terminal_unknown_or_invalid_requests`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA033 / 修改断言 | 默认 reason 的响应结局 | `defaulted[status] == aborted` | `defaulted[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |
| RA034 / 修改断言 | 已 aborted 的响应结局 | `existing[status] == aborted` | `existing[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/run_abandon.rs`

测试 / fixture：`abandon_keeps_the_assistant_binding_workspace_and_history_for_handover`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA035 / 修改断言 | 交接前 abandon 响应结局 | `abandoned[status] == aborted` | `abandoned[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/session_calls.rs`

测试 / fixture：`completed_session_execution_stays_waiting_until_delivery_settlement`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA036 / 修改断言 | 执行完成且 delivery pending 的相位 | `detail[state][status] == completed` | `detail[projection][phase] == settled` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/api/tests/session_calls.rs`

测试 / fixture：`completed_pending_wait_child_recovers_delivery_and_parent_after_restart`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA037 / 修改断言 | 子 Run 列表结局 | `child_projection[status] == completed` | `child_projection[projection][outcome] == done` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/application/kernel_runs/tests.rs`

测试 / fixture：`serial_probe_commits_artifacts_cost_and_phase_through_application`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA038 / 修改断言 | 串行 probe 完成 | `assert!(project_run(...).terminal)` | `assert!(project_run(...).phase == settled)` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/application/kernel_runs/tests.rs`

测试 / fixture：`abandon_waiting_probe_is_terminal_and_refuses_late_submission`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA039 / 修改断言 | 等待 probe 被放弃 | `assert!(project_run(...).terminal)` | `assert!(project_run(...).phase == settled)` | S19：新 phase/outcome 词汇 |

##### `rust/anchor-runner-host/src/application/liveness.rs`

测试 / fixture：`parked_or_unregistered_ready_runs_are_not_quiescent`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA040 / 修改断言 | RunStatus::Paused、worker=false | `RunLiveness::observed(status,false).is_quiescent() == false` | `RunLiveness::observed(status,false).is_quiescent() == true` | S21：Paused/BudgetStopped 统一为静止 |
| RA041 / 修改断言 | RunStatus::BudgetStopped、worker=false | `RunLiveness::observed(status,false).is_quiescent() == false` | `RunLiveness::observed(status,false).is_quiescent() == true` | S21：Paused/BudgetStopped 统一为静止 |

##### `rust/anchor-runner-host/src/application/run_view.rs`

测试 / fixture：`actions_cover_every_phase_waiter_and_outcome_with_and_without_worker`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA042 / 修改断言 | Phase::Executing、worker=false；AvailableActions.resume 分量 | `AvailableActions {stop:true,resume:false,abandon:true}` | `AvailableActions {stop:true,resume:true,abandon:true}` | S20：无 worker 的 Executing 可以 resume |

##### `rust/anchor-runner-host/src/application/run_projection.rs`

测试 / fixture：`inactive_states_reuse_control_guards_and_liveness`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA043 / 修改断言 | Ready、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA044 / 修改断言 | Ready、worker=false：注意项投影检查 | `projection.needs_attention == false` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA045 / 修改断言 | Ready、worker=false：静止检查读取来源 | `projection.quiescent == false` | `RunLiveness::observed(status,false).is_quiescent() == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA046 / 修改断言 | Running、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA047 / 修改断言 | Running、worker=false：注意项投影检查 | `projection.needs_attention == true` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA048 / 修改断言 | Running、worker=false：静止检查读取来源 | `projection.quiescent == false` | `RunLiveness::observed(status,false).is_quiescent() == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA049 / 修改断言 | Paused、worker=false：stop 表项 | `projection.actions.stop == true` | `projection.actions.stop == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA050 / 修改断言 | Paused、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA051 / 修改断言 | Paused、worker=false：注意项投影检查 | `projection.needs_attention == false` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA052 / 修改断言 | Paused、worker=false：静止检查读取来源 | `projection.quiescent == false` | `RunLiveness::observed(status,false).is_quiescent() == true` | S21：Paused/BudgetStopped 统一为静止 |
| RA053 / 修改断言 | WaitingCall、worker=false：resume 表项 | `projection.actions.resume == true` | `projection.actions.resume == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA054 / 修改断言 | WaitingCall、worker=false：stop 表项 | `projection.actions.stop == true` | `projection.actions.stop == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA055 / 修改断言 | WaitingCall、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA056 / 修改断言 | WaitingCall、worker=false：注意项投影检查 | `projection.needs_attention == false` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA057 / 修改断言 | WaitingCall、worker=false：静止检查读取来源 | `projection.quiescent == false` | `RunLiveness::observed(status,false).is_quiescent() == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA058 / 修改断言 | WaitingRecovery、worker=false：stop 表项 | `projection.actions.stop == true` | `projection.actions.stop == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA059 / 修改断言 | WaitingRecovery、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA060 / 修改断言 | WaitingRecovery、worker=false：注意项投影检查 | `projection.needs_attention == true` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA061 / 修改断言 | WaitingRecovery、worker=false：静止检查读取来源 | `projection.quiescent == false` | `RunLiveness::observed(status,false).is_quiescent() == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA062 / 修改断言 | BudgetStopped、worker=false：resume 表项 | `projection.actions.resume == false` | `projection.actions.resume == true` | S23：取消旧恢复/预算 resume 展示限制 |
| RA063 / 修改断言 | BudgetStopped、worker=false：stop 表项 | `projection.actions.stop == true` | `projection.actions.stop == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA064 / 修改断言 | BudgetStopped、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA065 / 修改断言 | BudgetStopped、worker=false：注意项投影检查 | `projection.needs_attention == true` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA066 / 修改断言 | BudgetStopped、worker=false：静止检查读取来源 | `projection.quiescent == false` | `RunLiveness::observed(status,false).is_quiescent() == true` | S21：Paused/BudgetStopped 统一为静止 |
| RA067 / 修改断言 | Stopped、worker=false：stop 表项 | `projection.actions.stop == true` | `projection.actions.stop == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA068 / 修改断言 | Stopped、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA069 / 修改断言 | Stopped、worker=false：注意项投影检查 | `projection.needs_attention == false` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA070 / 修改断言 | Stopped、worker=false：静止检查读取来源 | `projection.quiescent == true` | `RunLiveness::observed(status,false).is_quiescent() == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA071 / 修改断言 | Completed、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA072 / 修改断言 | Completed、worker=false：注意项投影检查 | `projection.needs_attention == false` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA073 / 修改断言 | Completed、worker=false：静止检查读取来源 | `projection.quiescent == true` | `RunLiveness::observed(status,false).is_quiescent() == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA074 / 修改断言 | Failed、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA075 / 修改断言 | Failed、worker=false：注意项投影检查 | `projection.needs_attention == true` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA076 / 修改断言 | Failed、worker=false：静止检查读取来源 | `projection.quiescent == true` | `RunLiveness::observed(status,false).is_quiescent() == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA077 / 修改断言 | Aborted、worker=false：pause 检查替换为 abandon | `assert!(!projection.actions.pause)` | `projection.actions.abandon == false` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA078 / 修改断言 | Aborted、worker=false：注意项投影检查 | `projection.needs_attention == true` | `(phase==waiting \|\| outcome∈{failed,aborted} \|\| (phase==executing && !active)) == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA079 / 修改断言 | Aborted、worker=false：静止检查读取来源 | `projection.quiescent == true` | `RunLiveness::observed(status,false).is_quiescent() == true` | S22/S24：动作按相位推导、移除 pending-control 展示 |

##### `rust/anchor-runner-host/src/application/run_projection.rs`

测试 / fixture：`pending_control_and_session_coordination_are_observed_once`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA080 / 修改断言 | active=true、requested=pause 的相位 | `projection.phase == pausing` | `projection.phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA081 / 修改断言 | active=true、requested=pause 的动作字段 | `assert!(!projection.actions.pause)` | `assert!(projection.actions.abandon)` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA082 / 修改断言 | active=true、requested=stop 的相位 | `projection.phase == stopping` | `projection.phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA083 / 修改断言 | active=true、requested=stop 的动作可见性 | `assert!(!projection.actions.stop)` | `assert!(projection.actions.stop)` | S22/S24：动作按相位推导、移除 pending-control 展示 |

##### `rust/anchor-runner-host/src/application/run_projection.rs`

测试 / fixture：`legacy_projection_serialization_preserves_ready_paused_and_stopped_bytes`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA084 / 修改断言 | RunStatus::Ready：serde_json::to_vec(...) == expected.as_bytes() | `{"phase":"ready","active":false,"control_requested":null,"terminal":false,"quiescent":false,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}` | `{"phase":"executing","reason":null,"active":false,"actions":{"stop":true,"resume":true,"abandon":true},"updated":null}` | S19：新 phase/outcome 词汇；S22/S24：动作按相位推导、移除 pending-control 展示；S26：updated 可空 |
| RA085 / 修改断言 | RunStatus::Paused：serde_json::to_vec(...) == expected.as_bytes() | `{"phase":"paused","active":false,"control_requested":null,"terminal":false,"quiescent":false,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}` | `{"phase":"waiting","waiting_for":"human","reason":"paused","active":false,"actions":{"stop":false,"resume":true,"abandon":true},"updated":null}` | S19：新 phase/outcome 词汇；S22/S24：动作按相位推导、移除 pending-control 展示；S26：updated 可空 |
| RA086 / 修改断言 | RunStatus::Stopped：serde_json::to_vec(...) == expected.as_bytes() | `{"phase":"stopped","active":false,"control_requested":null,"terminal":false,"quiescent":true,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}` | `{"phase":"waiting","waiting_for":"human","reason":"stopped","active":false,"actions":{"stop":false,"resume":true,"abandon":true},"updated":null}` | S19：新 phase/outcome 词汇；S22/S24：动作按相位推导、移除 pending-control 展示；S26：updated 可空 |

##### `rust/anchor-runner-host/src/application/tests.rs`

测试 / fixture：`pending_control_projects_active_tokens_with_stop_priority`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA087 / 修改断言 | 缺少 worker 的 missing record | `project_run(...).control_requested == None` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA088 / 修改断言 | 已登记 worker、未请求控制 | `project_run(...).control_requested == None` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA089 / 修改断言 | pause flag=true | `project_run(...).control_requested == Some(pause)` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA090 / 修改断言 | pause+cancellation flags=true；原 stop 优先 | `project_run(...).control_requested == Some(stop)` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA091 / 修改断言 | 移除 worker 后 | `project_run(...).control_requested == None` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |

##### `rust/anchor-runner-host/src/application/session_calls.rs`

测试 / fixture：`stopped_parent_cancels_wait_delivery (fixture) → active_parent_stop_cancels_wait_session_delivery_but_not_detach`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA092 / 修改断言 | active_parent=true，stop 后仍排空的父 Run | `project_run(parent,...).control_requested == Some(stop)` | `project_run(parent,...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |

#### fixture 关联测试

`start_and_pause_request` 直接或经 `start_and_pause` 影响以下既有测试；本表中的 fixture 断言只计一次，未重复算到每个调用者：

- `api/tests.rs::pause_resume_uses_same_frozen_run_after_graph_edit_and_service_object_restart`
- `api/tests/platform_contract.rs::busy_schedule_creates_no_run_or_source_metadata`
- `api/tests/platform_contract.rs::updated_tracks_persisted_pause_stop_and_resume_after_restart`
- `api/tests/schedules.rs::busy_due_schedule_skips_existing_admission_without_creating_a_run`
- `api/tests/run_abandon.rs::abandon_leaves_other_runs_and_their_recovery_entry_points_unchanged`

`application/session_calls.rs::stopped_parent_cancels_wait_delivery(true)` 的改动断言由 `active_parent_stop_cancels_wait_session_delivery_but_not_detach` 执行；false 分支的存储状态断言未改，关联测试为 `waiting_parent_stop_cancels_completed_pending_session_child_without_waking_parent`。

#### 动态表完整行变化核对

`application/run_projection.rs::inactive_states_reuse_control_guards_and_liveness` 的表元组列为 `(resume, stop, attention, quiescent)`；这里保留所有 10 行，避免把没修改的分量误报为修改。上文逐项 attention 新值使用实际表达式：Ready 还会经 `attention || status==Ready` 得到 true。

| RunStatus | 旧表元组 | 新表元组 | 实际新 attention |
| --- | --- | --- | --- |
| Ready | `(true, true, false, false)` | `(true, true, false, false)` | true |
| Running | `(true, true, true, false)` | `(true, true, true, false)` | true |
| Paused | `(true, true, false, false)` | `(true, false, true, true)` | true |
| WaitingCall | `(true, true, false, false)` | `(false, false, true, false)` | true |
| WaitingRecovery | `(false, true, true, false)` | `(false, false, true, false)` | true |
| BudgetStopped | `(false, true, true, false)` | `(true, false, true, true)` | true |
| Stopped | `(true, true, false, true)` | `(true, false, true, true)` | true |
| Completed | `(false, false, false, true)` | `(false, false, false, true)` | false |
| Failed | `(false, false, true, true)` | `(false, false, true, true)` | true |
| Aborted | `(false, false, true, true)` | `(false, false, true, true)` | true |

`application/run_view.rs::actions_cover_every_phase_waiter_and_outcome_with_and_without_worker` 保留 9 相位/等待者/结局 × 2 worker 的 18 组合。仅 Executing / worker=false 的 resume 从 false 改 true；其余 17 组合以及全部 stop/abandon 预期未改。

`application/liveness.rs::parked_or_unregistered_ready_runs_are_not_quiescent` 保留 10 旧状态 × 2 worker 的 20 组合。仅 Paused、BudgetStopped 的无 worker 静止预期从 false 改 true；所有有 worker 的非静止断言未改。

#### 新增测试（不混计既有断言修改）

文件：`rust/anchor-runner-host/src/application/legacy_run_view.rs`。测试：`every_legacy_status_maps_lossily_with_both_worker_observations`。

对下表每一旧状态分别执行 worker=false / true，精确断言 `(phase,waiting_for,outcome,reason,active)`；active 必须等于 worker。

| 旧 RunStatus | phase | waiting_for | outcome | reason | worker 覆盖 |
| --- | --- | --- | --- | --- | --- |
| Ready | executing | None | None | None | false、true |
| Running | executing | None | None | None | false、true |
| Paused | waiting | human | None | paused | false、true |
| Stopped | waiting | human | None | explanation | false、true |
| BudgetStopped | waiting | human | None | budget | false、true |
| WaitingRecovery | waiting | effect | None | explanation | false、true |
| WaitingCall | waiting | child | None | explanation | false、true |
| Completed | settled | None | done | None | false、true |
| Failed | settled | None | failed | explanation | false、true |
| Aborted | settled | None | aborted | explanation | false、true |

每组合另有七个独立断言：

1. `actions.stop == (phase==executing)`。
2. `actions.resume == (!worker && (phase==executing || waiting_for==human))`。
3. `actions.abandon == (phase!=settled)`。
4. `waiting_for` 字段是否存在与 waiter 是否存在一致。
5. `outcome` 字段是否存在与 outcome 是否存在一致。
6. 不存在旧 `status` 字段。
7. 不存在旧 `actions.pause` 字段。

此外单独断言 `Stopped` 无 error 时映射为 `Waiting::Human {reason:"stopped"}`。新增测试共 9 个源码 assert 宏（循环内 8 个，循环外 1 个），展开后 161 次断言。归属：新词汇、动作按相位推导、无 worker Executing resume、S23：取消旧恢复/预算 resume 展示限制。


#### Web 逐项断言

| 编号 | 文件 | 测试名 | 原断言 | 新断言 | 行为分组 |
| --- | --- | --- | --- | --- | --- |
| W1 | `apps/web/e2e/goose-native-trace.spec.ts` | real Rust Host and Goose retain active trace, canonical completion and recorded Plugin metadata after refresh | `expect(during.state.status).toBe('running')` | `expect(during.projection?.phase).toBe('executing')` | S19/S20：Run 新词汇/worker 观察 |
| W2 | `apps/web/e2e/goose-native-trace.spec.ts` | real Rust Host and Goose retain active trace, canonical completion and recorded Plugin metadata after refresh | `expect.poll(async () => (await (await request.get(\`${base}/runs/${run}\`)).json()).state.status).toBe('completed')` | `expect.poll(async () => (await (await request.get(\`${base}/runs/${run}\`)).json()).projection.outcome).toBe('done')` | S19/S20：Run 新词汇/worker 观察 |
| W3 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.state.status).toBe('running')` | `expect(detail.projection?.phase).toBe('executing')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W4 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.calls[0].status).toBe('running')` | `expect(detail.calls[0].projection.phase).toBe('executing')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W5 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(waitDetail.state.status).toBe('completed')` | `expect(waitDetail.projection?.outcome).toBe('done')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W6 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.state.status).toBe('completed')` | `expect(detail.projection?.outcome).toBe('done')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W7 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect(detail.calls[0].status).toBe('running')` | `expect(detail.calls[0].projection.phase).toBe('executing')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W8 | `apps/web/e2e/graph-calls-real.spec.ts` | real backend: author wait and detach calls, follow results and concurrent run chains | `expect((await json(request, \`${base}/runs/${detail.calls![0].run}\`)).state.status).toBe('completed')` | `expect((await json(request, \`${base}/runs/${detail.calls![0].run}\`)).projection.outcome).toBe('done')` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W9 | `apps/web/e2e/graph-calls.spec.ts` | mocked: show wait/detach child Runs, source links and active controls | `expect(page.getByRole('button', { name: '暂停' })).toBeVisible()` | `expect(page.getByRole('button', { name: '放弃', exact: true })).toBeVisible()` | S19/S22/S23/S24：子 Run phase/outcome 与三动作 |
| W10 | `apps/web/e2e/parallel-real.spec.ts` | real backend: paired authoring, simultaneous native nodes, joined artifacts and invalid-pair rejection | `expect(detail.state.status).toBe('completed')` | `expect(detail.projection?.outcome).toBe('done')` | S19/S20：Run 新词汇/worker 观察 |
| W11 | `apps/web/e2e/parallel.spec.ts` | mocked: show simultaneous branch execution and pending join in one Run | `expect(page.locator('.execution-node.state-running')).toHaveCount(2)` | `expect(page.locator('.execution-node.state-executing.is-live')).toHaveCount(2)` | S19/S20：Run 新词汇/worker 观察 |
| W12 | `apps/web/e2e/recovery.spec.ts` | unknown tool result resumes through the normal Agent context path | `expect(page.getByText('Agent 上次工具调用的结果未记录')).toBeVisible()` | `expect(page.locator('.canvas-head .pill', { hasText: '等待你处理：Agent 上次工具调用的结果未记录' })).toBeVisible()` | S19/S20：Run 新词汇/worker 观察 |
| W13 | `apps/web/e2e/recovery.spec.ts` | inactive running Run can be resumed from its existing detail page | `expect(page.locator('.execution-node.state-running')).toHaveCount(0)` | `expect(page.locator('.execution-node.is-live')).toHaveCount(0)` | S19/S20：Run 新词汇/worker 观察 |
| W14 | `apps/web/e2e/recovery.spec.ts` | inactive running Run can be resumed from its existing detail page | `expect(page.locator('.execution-node.state-waiting_resume')).toHaveCount(1)` | `expect(page.locator('.execution-node.state-executing')).toHaveCount(1)` | S19/S20：Run 新词汇/worker 观察 |
| W15 | `apps/web/e2e/recovery.spec.ts` | inactive running Run can be resumed from its existing detail page | `expect(page.getByText('宿主重启后尚未接续')).toBeVisible()` | `expect(page.locator('[aria-label="活动节点"]')).toContainText('等待接续')` | S19/S20：Run 新词汇/worker 观察 |
| W16 | `apps/web/src/ChannelsPage.test.tsx` | waits for the executor to exit before allowing retirement and binds it to the current Run | `expect(container.textContent).toContain('正在停止…')` | `expect(container.textContent).toContain('等待你处理：stopped')` | S19/S21：等待人/worker 排空/静止语义 |
| W17 | `apps/web/src/ChannelsPage.test.tsx` | does not infer resumability when controls are denied and uses human quiescence for retirement | `expect(button('退役实例').disabled).toBe(true)` | `expect(button('退役实例').disabled).toBe(false)` | S19/S21：等待人/worker 排空/静止语义 |
| W18 | `apps/web/src/RunDetailPage.test.tsx` | exposes the server-projected actions for %s | `expect(buttons.map(button => button.textContent)).toEqual(['停止', '继续'])` | `expect(buttons.map(button => button.textContent)).toEqual(['停止', '继续', '放弃'])` | S19/S22/S23/S24：统一等待文案/三动作 |
| W19 | `apps/web/src/RunDetailPage.test.tsx` | exposes the server-projected actions for %s | `expect(control.mock.calls).toEqual([['stop'], ['resume']])` | `expect(control.mock.calls).toEqual([['stop'], ['resume'], ['abandon']])` | S19/S22/S23/S24：统一等待文案/三动作 |
| W20 | `apps/web/src/RunDetailPage.test.tsx` | exposes the server-projected actions for %s | `expect(element.textContent).toContain(status === 'waiting_call' ? '等待调用结果' : status === 'ready' ? '等待执行' : '等待恢复上下文')` | `expect(element.textContent).toContain(label)` | S19/S22/S23/S24：统一等待文案/三动作 |
| W21 | `apps/web/src/RunDetailPage.test.tsx` | uses the projection for cursor and control hints despite conflicting legacy fields | `expect(element.querySelector('[aria-label="活动节点"]')?.textContent).toContain('宿主重启后尚未接续')` | `expect(element.querySelector('[aria-label="活动节点"]')?.textContent).toContain('等待接续')` | S19/S22/S23/S24：统一等待文案/三动作 |
| W22 | `apps/web/src/Timeline.test.ts` | keeps the restart message for a Run the host no longer owns | `expect(factsFor({ running: false, status: 'running', updated: '2026-09-29T10:30:00', projection: { phase: 'waiting_resume', active: false, control_requested: null, terminal: false, quiescent: false, needs_attention: true, actions: { pause: false, stop: true, resume: true }, } })).toEqual({ label: '运行状态', value: '宿主重启后等待接续', })` | `expect(factsFor({ running: false, updated: '2026-09-29T10:30:00', projection: { phase: 'executing', active: false, reason: null, updated: null, actions: { stop: true, resume: true, abandon: true }, } })).toEqual({ label: '运行状态', value: '宿主重启后等待接续', })` | S19/S20：Timeline phase/attention |
| W23 | `apps/web/src/Timeline.test.ts` | reports a resident segment by its own window, not the Run lifetime | `expect(factsFor({ running: false, status: 'stopped', activity: [] }, { resident: true, end: +new Date('2026-09-28T09:00:00') })).toEqual({ label: '这段时长', value: '暂无记录', end: '暂无记录', })` | `expect(factsFor({ running: false, activity: [], projection: { phase: 'waiting', waiting_for: 'human', active: false, reason: 'stopped', updated: null, actions: { stop: false, resume: true, abandon: true } } }, { resident: true, end: +new Date('2026-09-28T09:00:00') })).toEqual({ label: '这段时长', value: '暂无记录', end: '暂无记录', })` | S19/S20：Timeline phase/attention |
| W24 | `apps/web/src/Timeline.test.ts` | includes waiting recovery in the same attention filter used by the Timeline | `expect(entry.status).toBe('waiting_recovery')` | `expect(entry.status).toBe('waiting')` | S19/S20：Timeline phase/attention |
| W25 | `apps/web/src/parallel.test.ts` | shows both active branches, animates both selected inputs, and leaves join pending | `expect(nodes.filter(node => node.data.state === 'running').map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | `expect(nodes.filter(node => node.data.state === 'executing' && node.data.live).map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | S19/S20：Run 新词汇/worker 观察 |
| W26 | `apps/web/src/parallel.test.ts` | shows both active branches, animates both selected inputs, and leaves join pending | `expect(toFlowNodes(region, 'parallel', { ...state, active: {} }, observation).filter(node => node.data.state === 'running')).toEqual([])` | `expect(toFlowNodes(region, 'parallel', { ...state, active: {} }, observation).filter(node => node.data.live)).toEqual([])` | S19/S20：Run 新词汇/worker 观察 |
| W27 | `apps/web/src/parallel.test.ts` | projects native Rust branch cursors while leaving completed branches and join inactive | `expect(toFlowNodes(region, 'parallel', native, observation).filter(node => node.data.state === 'running').map(node => node.id)) .toEqual(['branch-a', 'branch-b'])` | `expect(toFlowNodes(region, 'parallel', native, observation).filter(node => node.data.state === 'executing' && node.data.live).map(node => node.id)) .toEqual(['branch-a', 'branch-b'])` | S19/S20：Run 新词汇/worker 观察 |
| W28 | `apps/web/src/parallel.test.ts` | keeps retained %s branch cursors quiet instead of rendering execution | `expect(nodes.filter(node => node.data.state === phase).map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | `expect(nodes.filter(node => node.data.state === view.phase).map(node => node.id)).toEqual(['branch-a', 'branch-b'])` | S19/S20：Run 新词汇/worker 观察 |
| W29 | `apps/web/src/parallel.test.ts` | keeps retained %s branch cursors quiet instead of rendering execution | `expect(nodes.filter(node => node.data.state === 'running')).toEqual([])` | `expect(nodes.filter(node => node.data.live)).toEqual([])` | S19/S20：Run 新词汇/worker 观察 |
| W30 | `apps/web/src/parallel.test.ts` | keeps unknown Run observations quiet while retaining definition-only editor nodes | `expect(unknown.filter(node => node.data.state === 'running')).toEqual([])` | `expect(unknown.filter(node => node.data.live)).toEqual([])` | S19/S20：Run 新词汇/worker 观察 |

参数化断言的具体取值（同一表达式的每个实例）：

| 文件/测试 | 原预期 | 新预期 | 行为分组 |
| --- | --- | --- | --- |
| RunDetailPage / exposes… / waiting_call | 按钮停止、继续；调用stop/resume；等待调用结果 | 按钮停止、继续、放弃；调用stop/resume/abandon；waiting/child 显示等待子任务 | S19/S22/S23/S24：三动作/等待词汇 |
| RunDetailPage / exposes… / ready | 按钮停止、继续；调用stop/resume；等待执行 | 按钮停止、继续、放弃；调用stop/resume/abandon；executing+inactive 显示等待接续 | S19/S20/S22：三动作/Ready合并 |
| RunDetailPage / exposes… / waiting_recovery | 按钮停止、继续；调用stop/resume；等待恢复上下文 | 按钮停止、继续、放弃；调用stop/resume/abandon；waiting/effect+reason 显示等待核查效果：工具结果未记录 | S19/S22/S23/S24：三动作/等待词汇；此fixture故意独立给动作事实以检查UI不自行授权 |
| RunInspector / uses… / waiting_resume / node-summary | 等待接续 | executing+inactive 仍显示等待接续 | S20：worker观察 |
| RunInspector / uses… / waiting_resume / call-records | 本节点：等待接续 | 本节点：等待接续 | S20：worker观察 |
| RunInspector / uses… / waiting_recovery / node-summary | 等待恢复上下文 | 等待核查效果：工具结果未记录 | S19：统一reason |
| RunInspector / uses… / waiting_recovery / call-records | 本节点：等待恢复上下文 | 本节点：等待核查效果：工具结果未记录 | S19：统一reason |
| RunInspector / uses… / paused / node-summary | 已暂停 | 等待你处理：paused | S19/S21：Human统一 |
| RunInspector / uses… / paused / call-records | 本节点：已暂停 | 本节点：等待你处理：paused | S19/S21：Human统一 |
| parallel / keeps retained… / waiting_resume / branch identities | 两分支state=waiting_resume | 两分支state=executing，inactive | S20：worker观察 |
| parallel / keeps retained… / waiting_resume / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |
| parallel / keeps retained… / waiting_recovery / branch identities | 两分支state=waiting_recovery | 两分支state=waiting，waiting_for=effect | S19：等待词汇 |
| parallel / keeps retained… / waiting_recovery / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |
| parallel / keeps retained… / paused / branch identities | 两分支state=paused | 两分支state=waiting，waiting_for=human，reason=paused | S19/S21：Human统一 |
| parallel / keeps retained… / paused / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |
| parallel / keeps retained… / stopped / branch identities | 两分支state=stopped | 两分支state=waiting，waiting_for=human，reason=stopped | S19/S21：Human统一 |
| parallel / keeps retained… / stopped / live identities | state=running的节点为空 | live=true的节点为空 | S20：worker观察 |

fixture 的间接断言变化与未覆盖边界：

- `Timeline.test.ts / includes waiting recovery…`：原通过手工切换 needs_attention=false 检查attention；新将 waiting/effect 切换为 settled/done 后仍断言false，禁止隐藏的attention旗标。原true/false断言均保留。
- `RunDetailPage.test.ts / shows recovery guidance…`：原恢复专用文案来自waiting_recovery分支；现只来自projection.reason。通过state.error带旧文案但projection无该reason验证不泄露旧字段；原正反文案断言保留。
- `ChannelsPage.test.ts / does not infer resumability…`：旧stopped但quiescent=false的fixture现在映射waiting/human+inactive，按新语义允许退役（disabled=true→false）；Run controls仍全部拒绝。属于Human静止统一的可见行为变化。
- `e2e/recovery.spec.ts / unknown tool result resumes…`：旧waiting_recovery fixture允许resume；为保留原resume成功操作验收，新fixture显式waiting/human并携同一恢复说明。它只验证人等待后的接续，不证明effect等待可以resume；effect按新动作规则不允许resume，停止条件后未新增独立effect组件测试。恢复副作用、产物、实际provider未由这个mock验证。
- `RunDetailPage / exposes…` 的三个fixture都故意提供stop/resume/abandon=true，验证UI只读Host动作；不代表这些phase真实Host都会授权这三项。
- 其余typed Run/CallRecord、mocked e2e的projection只将旧Run词汇与flags迁为新字段；Session Turn、native branch status、listener、节点完成/中断事实不迁移。原非状态断言全部保留。
- `Timeline` 的finished类选择器迁为settled（原宽度、时间、几何断言未变）；AST清单只统计expect表达式，selector绑定在变量赋值中的变化需按此补记。


## R2 审查结论（2026-10-19）

审查方复现 API 失败（10+ 项，均与 pause 语义或旧状态字段相关），内核 diff 为空。R2 未验收。

1. **pause→stop 是任务书的设计错误，不是实现问题。** 旧 pause 的含义是"当前节点做完后停在边界"，这是宿主调度意图，不是一个状态。收敛的对象是状态词汇，不是控制动作：pause 应保留为动作（Executing 且有 worker 时可用，由宿主在节点边界提交 stop），结果状态是 waiting / human / reason=paused。S25 作废。
2. **测量的判定标准本身达不到。** conversations 一个消费者在基线就占 59/82 点，几乎全是 Session、授权、交接和投递，宪法从未打算消除它们；无论读模型如何设计，所选消费者都不可能减少 50%。P、R、R2 三次合起来的结论是：写入与生命周期一侧的定律成立；Host 读侧的复杂度主要在 Session/授权层，不随运行时收敛而消失。
3. 因此不再继续读侧探针。是否全量迁移改为按"统一定律、崩溃与计费正确性、删除旧 Runner"的价值决定，而不是按"Host 变小"决定。

**用户决定（2026-10-19）**：全量迁移，见[全量迁移任务书](constitution-migration-task.md)。

## M1：完成 R2 对外词汇（2026-10-19）

按 [全量迁移任务书](constitution-migration-task.md) 交回 M1。pause 恢复为独立宿主控制：旧 Run 保持原边界暂停；Kernel Run 持久保存意图，经事件在当前节点结果/成本提交后、下一派发前提交 Human stop，reason=paused。重启先恢复端口已有完成事实；最终节点已结算就保留 Done。stop 仍立即取消。控制与边界消费共用锁，旧意图清理不删除后来接受的意图。未修改 Kernel 或生产 Validator，未增加执行标志轮询。

HTTP、Pilot、Web 共用四动作；Session call 通用 pause/resume 仍拒绝。物理静止不等于 Session 可交接：conversation 暂停或预算等待时保留当前轮占有，显式 stop 或结算后才允许交接/删除；原 409、产物、不重放与投递断言保持。已删除仅供旧调用点使用的 `observed_liveness` 包装，直接复用同一 RunView 与静止规则。

本地验证：workspace 1922 通过、0 失败、115 既有忽略；Web 290 通过、build 通过，fmt/clippy/C1 通过。结果与命令见 [M1 验收](../experiments/constitution-host-preflight/m1/verification.md)。真实 Goose/Provider、native 浏览器端到端未执行，不作为已验收能力。M2–M5 尚未开始；不再以读侧判断点下降 50% 决定迁移，R/R2 历史测量保持原样。

### S 候选

S25 按任务书作废；无新增 S 编号。其余 **7 条待批**，未擅自记为批准。

| 编号 | 当前结论 / 待审查边界 |
| --- | --- |
| S19 | 三相位、waiter/outcome/reason 取代旧 Run 状态与专用提示；旧客户端字段破坏，Ready 进入接续/注意展示 |
| S20 | Executing 无 worker 可恢复驱动，旧 Ready 与孤立 Running 共用规则 |
| S21 | Human 无 worker 统一物理静止；conversation 当前轮的占有/交接约束留在 Session 层，原暂停 409 保留；准入、退役与清理的其余影响仍待审查 |
| S22 | stop 仅 Executing；未结算增加 abandon；M1 按已批准修正恢复 pause=(Executing && worker) |
| S23 | Human 无 worker 可 resume，Child/Effect 不可；未迁旧预算写入守卫仍可能拒绝，展示与写入边界待审查 |
| S24 | 无 pausing/stopping/control_requested 展示，请求未落相位前仍显示 Executing；实际暂停接受由 operation=pause 回执确认 |
| S26 | 缺 Kernel 时间观察为 updated:null；旧 mtime 来源保留，时间不参与控制 |

### 断言修改

计数为既有期望表达式/目标位置，参数展开在逐项登记内列清：Rust **26 处**（20 处断言期望、6 处轮询目标），Web **4 处**；fixture 仅增补 actions.pause 的改动另列。新增 Rust 8 项、Web 5 项测试，无业务断言放宽。RSI HTTP MCP 测试仅显式初始化 workspace 既用的 ring crypto provider，原工具发现和冻结证据读取断言不变。

| 登记 | 测试 / 旧 → 新 / 依据 |
| --- | --- |
| MR01 | api/tests.rs / paused_wait_child_resumes_independently_then_releases_parent / 轮询；child.status Stopped → Paused；M1 纠正 S25，恢复边界暂停 |
| MR02 | 同上 / 结果断言；child.status Stopped → Paused；同上；恢复和父子释放断言保留 |
| MR03 | api/tests.rs / start_and_pause_request / 响应 operation；stop → pause；M1 独立控制动作 |
| MR04 | 同上 / 响应 alias；alias=true → 不含 alias；M1 取消别名协议 |
| MR05 | 同上 / 存储状态；Stopped → Paused；M1；原 results.len()==1 不变 |
| MR06 | api/tests.rs / waiting_parent_rejects_child_grant_drift_then_restores_same_child_without_replay；child_before.status Stopped → Paused；M1；授权拒绝、产物、首结果与不重放断言不变 |
| MR07 | api/tests/conversations.rs / conversation_resume_is_not_blocked_by_another_session_on_the_same_graph / 轮询；Bob Stopped → Paused；M1；下一轮仍 409、原结果与调用次数不变 |
| MR08 | 同文件 / superseded_conversation_cannot_resume_or_recover_its_wait_child / 轮询；暂停后 child Stopped → Paused；M1；随后显式 stop 仍 Stopped，旧轮恢复拒绝不变 |
| MR09 | 同文件 / superseded_conversation_keeps_detached_child_independent / 轮询；暂停后 detached child Stopped → Paused；M1；随后显式 stop 及 detach 边界不变 |
| MR10 | api/tests/kernel_probe.rs / kernel_read_view_http_tracks_running_human_wait_and_settlement_with_host_time / Executing actions；stop/resume/abandon 原值保留，增加 pause=true；M1；有 worker 的 Executing 可暂停 |
| MR11 | 同上 / Waiting actions；原三动作保留，增加 pause=false；M1；等待时不可暂停 |
| MR12 | 同上 / Settled actions；原三动作保留，增加 pause=false；M1；结算时不可暂停 |
| MR13 | application/legacy_run_view.rs / every_legacy_status_maps_lossily_with_both_worker_observations；actions 无 pause → pause==(executing && worker)；M1；10 状态 × 2 worker，共 20 个参数实例 |
| MR14 | application/run_projection.rs / legacy_projection_serialization_preserves_ready_paused_and_stopped_bytes / Ready；精确字节 JSON actions 增加 pause:false；M1；其余字段字节不变 |
| MR15 | 同上 / Paused；精确字节 JSON actions 增加 pause:false；同上 |
| MR16 | 同上 / Stopped；精确字节 JSON actions 增加 pause:false；同上 |
| MR17 | application/run_view.rs / actions_cover_every_phase_waiter_and_outcome_with_and_without_worker；AvailableActions 增加 pause==(Executing && worker)；M1；9 相位/waiter/outcome × 2 worker，18 个参数实例 |
| MR18 | api/runs/tests.rs / list_and_detail_share_status_control_and_attention_projection / phase 参数期望；ready/waiting_call/waiting_resume/waiting_recovery/budget_stopped/completed → executing/waiting/executing/waiting/waiting/settled；S19/S20；六实例见逐项登记 |
| MR19 | 同上 / resume 参数期望；WaitingCall、WaitingRecovery true→false；BudgetStopped false→true，其余不变；S23；只改动作投影期望，旧写入守卫未动 |
| MR20 | 同上 / stop 参数期望；WaitingCall、WaitingRecovery、BudgetStopped true→false，其余不变；S22；Executing 才可 stop |
| MR21 | 同上 / attention 断言；projection.needs_attention → waiting 或 executing+inactive 的事实推导；Ready/WaitingCall false→true；S19/S20；同时断言旧字段不存在，六实例见逐项登记 |
| MR22 | 同文件 / incomplete_run_identity_does_not_offer_inactive_controls / detail；三个动作 false → 四个动作 false，增加 abandon=false；S22；身份拒绝保持全部动作不可用 |
| MR23 | 同上 / list；三个动作 false → 四个动作 false，增加 abandon=false；同上；不放宽授权拒绝 |
| MR24 | tests/standard_host / standard_host_runs_op_without_goose_model_or_runtime_switch：失败断言 state.status!=failed → projection.outcome!=failed；S19，产物断言保留 |
| MR25 | 同上 / 完成轮询 state.status=completed → projection settled/done；S19 |
| MR26 | 同文件 / old_state_root_still_allows_op_without_becoming_goose_recovery：完成轮询 state.status=completed → projection settled/done；S19，旧事实断言保留 |
| MW01 | platform-rust / native Rust Host…：Run pill 已暂停 → 等待你处理：paused；M1/S25 作废 |
| MW02 | 同上 / scheduled Run：status=completed → projection settled/done；S19/S20 |
| MW03 | pilot-rust / Goose Pilot controls…：control_requested=pause → projection executing+active；新增独立 operation=pause 工具回执检查；S24/M1 |
| MW04 | 同上 / Run pill：已暂停 → 等待你处理：paused；M1/S25 作废 |

逐项参数、原/新值与 fixture schema 清单：[Rust](../experiments/constitution-host-preflight/m1/rust-assertions.md)、[Web](../experiments/constitution-host-preflight/m1/web-assertions.md)。R2 登记仍代表当时现场，本节覆盖已作废的别名断言。

### 删除测试映射

| 删除数量 | 对应新路径或理由 |
| --- | --- |
| 0 | 未删除测试、未新增 ignore；暂停/恢复、stop 优先、重启两种完成事实、HTTP 双后端三阶段、Pilot 独立控制和 Web effect 控件为新增覆盖 |

## 尚未完成

- 并行分支内的模块作用域。
- 全量 Host 迁移按 M1–M5 执行，H3–H6 阶段划分已被取代；M1 本地实现与验证交回，S19–S24、S26 待审查。M2 的并行/Call/助手、结果与恢复决定/删除，M3 默认切换，M4 旧 Runner 删除，M5 测量仍未完成。全局 C2 尚未验收，16 save / 36 mutation 为旧路径棘轮起点。
- 旧 Runner 的删除。
- M1 的本地验收不替代真实 Goose/Provider、部署与生产数据离线迁移；W4 未决定。当前实现缺时间观察为 null，不伪造历史时间。
- 共享 conversation lease / Library 读锁 / `PROCESS_ENV_LOCK` 的既有多测试 flaky（见"待处理与观察"）。
