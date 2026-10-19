# Run 访问边界

## 当前实现

F0 为 `e3a5778`，首个/第二个 F1 切片为 `06f7b8a`、`c21b382`，G0 `250435d` 经 `3b7d1d9` 合入。`application/run_access` 统一 Run 读取、状态规则及 HTTP/Pilot/通道兼容序列化。Graph/会话用例只消费安全视图、谓词及意图操作；不可拆开的执行/保存协议留在门面内部，避免暴露原始 record 作为中间产物。持久格式、对外词汇、权限、锁顺序及写入顺序不变；F1 没有修改既有测试断言。

最终读取切片补齐 Graph/会话/恢复/删除规则和 Pilot Session 兼容输出；业务围栏从 224 经 193、99 降为 5，仅 `application/recovery.rs` 的写入协议仍待 F2 收口。`RunView` 只提供新相位、活动观察、持久更新时间、只读 Graph 进度及谓词；record 为私有 Cow，既不 Deref 也没有原始 record getter，不公开 status/error/cursor/recovery/recovery_submissions。`read_run`、`run_views`、`runs_for_graph` 返回拥有数据的安全视图；原始枚举仅在门面内部可用，旧 `records()` 只在测试编译保留。outcome 保留 escalated/unknown 契约，master 现有状态仍仅映射为 done/failed/aborted。读取完整验收与剩余边界见文末，围栏数不是规则度量。

[围栏测试](../rust/anchor-runner-host/tests/run_access_fence.rs) 扫描 `anchor-runner-host/src/**/*.rs`，对 `RunStatus`、`GraphRunRecord`、`RunStore`、`FileRunStore`、`GraphRunner`、`RunCursor` 六种内部类型的精确标识符逐次计数。[业务基线](../rust/anchor-runner-host/run-access-fence.txt) 是逐文件上限：超过上限或清单外业务文件出现引用会失败；下降或删除文件时提示同步降低清单，不自动改写它。

扫描排除 `tests/`、`*tests.rs` 和精确 `#[cfg(test)]` 所保护的项，包括外部测试模块；跳过该项后恢复扫描，不丢弃同文件后续生产代码。注释和字面量不计数，导入、完整路径、别名声明和宏输入中的类型标识符计数。不展开宏或执行名称解析，不追踪别名使用、glob 导入和推断类型；因此围栏不是所有直接字段读写的静态证明，F1/F2 仍须迁移这些路径并封装内部可变状态。

## 运行时范围

永久允许范围固定在测试中，不由业务基线扩展：

| 范围 | 归属理由 |
| --- | --- |
| `application/run_access.rs`、`application/run_access/` | Run 访问、规则、意图操作与兼容序列化的归属边界；没有新增其他业务豁免 |
| `execution.rs` | 共享 Runner 的执行装配与调用 |
| `main.rs` | 宿主、CLI 与 Runtime 的组合根 |
| `node_host.rs`、`node_host/` | NodeExecutionPort 的实现与装配；根文件和同名目录共同形成一个运行时模块，不另豁免业务路径 |
| `run_data.rs` | Runtime 产物及历史数据访问 |

`anchor-graph-host` 整个 crate 属于运行时侧，不在本围栏范围内。其余 Host 文件默认为业务侧，包括当前没有类型标识符、但仍通过推断类型读写 Run 的文件。

## HTTP 字节基线

[新增 HTTP 回归](../rust/anchor-runner-host/src/api/tests/run_access_json.rs) 固定 Run 身份与时间，逐字节比较 `/runs` 的原始响应，不先解析或重新序列化 JSON。覆盖十种既有状态及其非活动投影，包含控制动作、终态、静止、注意提示和字段顺序。等待恢复的样本保留有效 cursor 与待处理 attempt。

当前序列化保留字段插入顺序。F1 不得仅验证 JSON 对象等价，或在将状态序列化移入门面时调整字段次序。新增基线不替代后续详情、Pilot、活动执行、并发控制和恢复路径的原有断言与专项核查。

[详情/abandon 字节回归](../rust/anchor-runner-host/src/application/run_access/http.rs) 固定十种状态的完整详情与 abandon 原始序列化；[Pilot 字节回归](../rust/anchor-runner-host/src/application/run_access/tests.rs) 固定 Run 列表/详情及十种状态的 Session retained-record 输出。record 部分与重构前直接序列化的字段顺序逐字节比较，不用解析后的对象等价替代。HTTP 的更新时间继续取文件元数据，不改为 `RunView::updated()` 的持久字段；后者保留为只读契约并有独立测试。

## 规则度量：全 Host 读取规则

迁移前固定为 `e3a5778`，来源均为当时 `src/` 相对路径与行号。统计独立判断体，不统计 helper 调用、类型引用或状态赋值；既有 `RunStatus::is_terminal` 调用不算重复实现。1→1 表示归属收口，不虚报减少；新增相位 0→1 不改 wire。复合规则与子规则存在包含关系，以下分组不能相加冒充无重叠总量。Runtime 内核与 NodeExecutionPort 的执行规则不属于业务重复实现统计。

| 谓词/规则 | master 原语义与来源（`src/` 相对路径） | 迁移前 → 当前独立实现数 |
| --- | --- | --- |
| `RunView::phase` | Ready/Running→executing；Paused/Stopped/BudgetStopped→waiting human；WaitingCall→waiting child；WaitingRecovery→waiting effect；其余 settled | 0 → 1（新增内部视图映射，不改 wire） |
| `is_settled` | 仅 Completed/Failed/Aborted；Runtime 权威集合与 admission 原枚举补集收为一处，见下文 | 2 → 1 |
| `is_quiescent` | 无执行登记且终态或 Stopped；Paused/BudgetStopped 不算静止；`application/liveness.rs:32` | 1 → 1 |
| `needs_attention` | 既有投影 phase 为 failed/aborted/waiting_resume/waiting_recovery/budget_stopped；活动 stop/pause 的临时 phase 不提示；`application/run_projection.rs:89` | 1 → 1 |
| `can_pause` | active、非终态、非 Session 调用、无待处理控制；`application/run_projection.rs:99` | 1 → 1 |
| `can_stop` | active 时尚未请求 stop；inactive 时复用状态校验；`application/run_projection.rs:100` | 1 → 1 |
| `can_resume`（观察） | inactive、非 Session 调用、预算校验通过，且普通校验通过或有待恢复 attempt；`application/run_projection.rs:83`。身份、助手、放弃意图、后继等额外门槛仍由同一门面投影按原次序补查 | 1 → 1 |
| `validate_inactive_control` | 依次检查 WaitingRecovery 的非 stop、终态、inactive pause；保留原错误及顺序；`application/control.rs:229` | 1 → 1 |
| `validate_resume_budget` | BudgetStopped 必须显式预算策略；`application/control.rs:245` | 1 → 1 |
| `validate_control_identity` | metadata 与冻结 record 的 Graph digest 相同；`application/control.rs:208`、`application/recovery.rs:44` | 2 → 1 |
| `can_abandon` | Completed/Failed 不允许，其余允许；`application/abandon.rs:204`、`:301`；原 Aborted 幂等响应和并发结果分类保留 | 2 → 1 |
| `legacy_status` | API 原十种状态词汇；`api.rs:227`，Pilot 的 serde 状态词汇相同 | 显式映射 1 → 1（生成点移至门面） |

### 重复状态与恢复判断

`T` 表示 Completed/Failed/Aborted。来源扫描覆盖 Host 全部业务生产文件，排除测试与既定 Runtime 范围；原始状态判断位置见[扫描证据](../.local/run-access-facade-20261019/f1-final/before-state-sites.log)。多入口不同短路与错误顺序仍由用例保留，只有共同判断体收敛。

| 已收敛规则 | 原位置 | 迁移前 → 当前独立实现数 |
| --- | --- | --- |
| `has_stopped_execution`：状态为 `T ∪ {Stopped}` 或其补集，不含执行登记 | `application/control.rs:19`、`application/session_calls.rs:103`、`api/wecom.rs:858`、`api/turns.rs:33`、`api/turns.rs:51` | 5 → 1 |
| `can_dispatch`：detached Run 仅 Ready 可派遣/恢复 | `application/dispatch.rs:51`、`application/dispatch.rs:78` | 2 → 1 |
| `waits_for_invocation`：WaitingCall 与 cursor node/invocation 共同匹配 | `application/control.rs:71`、`application/dispatch.rs:172` | 2 → 1 |
| `waits_for_children`：是否等待子 Run，含 stop/abandon 与释放登记后复核 | `application/control.rs:123`、`application/control.rs:132`、`application/abandon.rs:281`、`application/dispatch.rs:234` | 4 → 1 |
| `has_succeeded`/`succeeded`：成功必须 Completed；不等于完整投递许可 | `application/conversations.rs:171`、`application/dispatch.rs:149`、`application/session_calls.rs:308`、`application/session_calls.rs:425`、`api/files.rs:196`、`api/wecom.rs:710` | 6 → 1 |
| `recovery_target`：cursor、attempt、WaitingRecovery，首次校验按原因映射、租约内复核统一冲突 | `application/recovery.rs:104`、`application/recovery.rs:206` | 2 → 1 |
| `recorded_recovery_decision`：同 invocation/attempt 查重 | `application/recovery.rs:56`、`application/recovery.rs:91` | 2 → 1 |
| `has_pending_recovery`：等待恢复且有 pending attempt | `application/control.rs:177`、`application/run_projection.rs:87` | 2 → 1 |
| `occupies_conversation_execution`：active 或 Ready/Running | `application/conversations.rs:708`、`api/wecom.rs:693` | 2 → 1 |
| `channel_turn_status`：普通/助手结算；助手 Completed 不结束 Turn | `application/conversations.rs:123`、`application/conversations.rs:160` | 2 → 1（保留 assistant 参数） |
| `is_abandoned`：已 Aborted；入口/结束/启动复核 | `application/abandon.rs:205`、`application/abandon.rs:291`、`application/abandon.rs:349` | 3 → 1（复用 settled + can_abandon） |
| `can_abandon`：非 Completed/Failed | `application/abandon.rs:221`、`application/abandon.rs:301` | 2 → 1 |
| `validate_control_identity`：元数据与冻结 Graph 一致 | `application/control.rs:208`、`application/recovery.rs:44` | 2 → 1 |

以上 13 条去重规则，独立判断体 **36 → 13**；只统计本表列明的原规则体，不含后述单点规则、相位或 Runtime 权威终态集合。

### 单点规则与复合用例

| 当前权威实现 | 迁移前来源/保留语义 | 迁移前 → 当前独立实现数 |
| --- | --- | --- |
| `is_settled` | Runtime 既有终态权威 + `application/admission.rs:44` 的七状态补集；所有既有 is_terminal 调用现在委托同一谓词 | 2 → 1 |
| `channel_progress_is_settled` | `api/wecom_progress.rs:373`；T/Stopped/BudgetStopped，missing 为 true；无 run 仍为 false | 1 → 1 |
| `ChannelReplyRun::from_record` 的 unsuccessful 投影 | `api/wecom.rs:692`、`api/wecom.rs:744`；Failed/Aborted/Stopped，两类 reply 共用 | 2 → 1 |
| `can_continue_session_call` | `application/session_calls.rs:308`；Completed 先短路，Failed/Aborted/BudgetStopped 阻止，其余继续 | 1 → 1 |
| `needs_recovery_replay` | `application/recovery.rs:69`；已存同一决定且 Running、未登记才 kick | 1 → 1 |
| `ensure_wait_closure_settled` | `application/conversations.rs:389`；保留登记、wait 子图闭包；handover 不允许 missing child、Graph cascade 允许 | 1 → 1 |
| `successor_reads_predecessor` | `application/conversations.rs:581`；missing successor 保留前驱；终态/已有后继不再读；首 invocation 注入证据 | 1 → 1 |
| `visible_active_nodes` | `application/graphs.rs:154`；仅活动 Running，排除助手 WAIT_INPUT，含活动并行 cursor | 1 → 1 |
| `executable_assistant_binding` | `assistant_plugins.rs:274`；active cursor 与 input commit 顺序查找，缺绑定原错误 | 1 → 1 |
| `latest_assistant_binding` | `assistant_plugins.rs:458`；cursor 优先，结果按 sequence 回退 | 1 → 1 |
| `delete_inactive_run` | `application/deletion.rs:244`；单 Run 所有权、入向 Graph call、Artifact 身份与清理；不强加 Graph cascade 静止条件 | 各 1 → 1 |

控制完整许可仍包含身份、现有助手、abandon intent、Session 协调与前驱约束；`project_run` 统一对外操作投影，写前核验继续使用同一谓词和原用例顺序。终态、静止、成功、已投递是不同事实，不以一个宽泛布尔量替代。

后续不可把相同子条件误当成完整规则相同：Session 投递先检查 active，文件读取先判断 Completed；普通渠道与常驻助手结算对 Completed 的处理不同；恢复前置验证与租约内复核的错误顺序不同；Graph 级联删除与单 Run 删除、等待闭包交接的缺失记录处理也不同。这些差别必须封在按意图命名的谓词/校验中，不改变原短路与错误顺序。

## 验证与剩余边界

- 最终读取切片：38 个门面定向回归通过；Rust workspace 1,993 通过、0 失败、122 忽略，严格 workspace/all-target clippy、fmt、Web 306 测试及构建通过。原始日志见[最终读取证据](../.local/run-access-facade-20261019/f1-final/README.md)。未新增真实 Provider 或生产业务验收。
- G0 三项残留见[Runtime 标准回归](runtime-contract-tests.md)。干净 target 首轮暴露标准构建遗漏联网 helper 与 MCP example，补全标准命令另交 `76f77ef`、`cff7001`；ACP 15 项及受影响四套 MCP 的 10 项串行复跑均通过。完整 ignored 首轮为 107/12/0，其中 9 项明确缺 example；结合补齐后的定向复跑，119 项的最终分类为 116 通过、仅 G0 三项失败、0 忽略，没有新增产品失败。这不是一次全绿命令，也不隐去环境失败。标准入口自身 fail-fast 于 G0 模型切换项（64/1/0），后续覆盖来自完整 ignored 命令。
- F2 尚需迁入 `application/recovery.rs` 最后一段按意图记录决定的写入协议，随后将 store 访问限定在门面内部；不得借类型重命名、扩大豁免或暴露原始 record 降低围栏数。Goose 三项 fixture/断言修正必须在 F1 完成后单独提交，并说明理由。
