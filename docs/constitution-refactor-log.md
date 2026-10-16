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

## 语义变更记录

| 编号 | 场景 | 旧行为 | 新行为 | 理由 |
| --- | --- | --- | --- | --- |
| S1 | 节点提交了非法路由 | 整个 Run 失败 | 打回给提议者修正（`RouteInvalid`） | 宪法：可修正的问题返回提议者，不终止任务 |
| S2 | 所有节点都因轮数上限而停止 | Run 状态为 `Stopped`，可以 resume，但 resume 后立即再次停止 | 结算为 `Escalated`，附停止原因 | 旧状态名义上可恢复，实际上无事可做；新状态如实表达"需要人决定" |
| S3 | 对已 `Aborted` 的 Run 再次 abandon | 幂等成功，返回同一结果 | 拒绝（`TaskSettled`） | 已结算不可逆，不需要为幂等单独设规则；调用方可通过读取状态得到同样信息 |
| S4 | 退役之后再 abandon（PoC E2 发现） | 允许 | 待定：退役属于 Session 指针，尚未迁移 | 需要用户决定 |

S1、S2 在差分测试中被显式映射（S2：旧 `Stopped` 且存在停止原因 ⇔ 新 `Escalated`）。S1 未进入差分测试，因为路由脚本只产生合法路由。

## 尚未完成

- fanout/join、Op.call 在新校验器中的表达。
- Host 侧接入：调度器、执行者端口、`NodeCompletion` 和 `RunStatus` 的 47 处使用方。
- 旧 Runner 的删除。
- 测量探针与原因码投影接入。
