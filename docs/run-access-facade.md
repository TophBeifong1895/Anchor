# Run 访问边界

## 当前实现

F0 已提交为 `e3a5778`，同时包含宪法文件与 F0–F2、G0 两份任务书。F1 首个读取切片已落在 `application/run_access`：新相位映射、观察谓词、静止判定及控制状态校验收至门面，HTTP 列表与 Pilot Run 读取由门面生成既有 JSON。持久格式、对外词汇、权限、锁顺序及写入顺序不变；没有修改既有测试断言。

这不是 F1 完成结论。第二个读取切片迁入 HTTP 详情/abandon 序列化、渠道启动/回复/进度读取、助手上下文与 Plugin binding 读取；业务围栏计数从 224 经 193 降为 99，仍有 9 个业务文件保留内部类型引用。`RunView` 提供相位、活动观察、Graph 拓扑与执行进度及状态谓词，不公开原始 record、status、error、cursor 或 recovery 字段；新增拥有数据的只读加载入口，Graph/会话的重型读写用例仍待迁移。outcome 契约保留 escalated/unknown，master 现有状态仅映射为 done/failed/aborted。更新时间与完整读取契约仍需补齐，不能把切片或围栏通过视为全部读取、规则与写入已收敛。

[围栏测试](../rust/anchor-runner-host/tests/run_access_fence.rs) 扫描 `anchor-runner-host/src/**/*.rs`，对 `RunStatus`、`GraphRunRecord`、`RunStore`、`FileRunStore`、`GraphRunner`、`RunCursor` 六种内部类型的精确标识符逐次计数。[业务基线](../rust/anchor-runner-host/run-access-fence.txt) 是逐文件上限：超过上限或清单外业务文件出现引用会失败；下降或删除文件时提示同步降低清单，不自动改写它。

扫描排除 `tests/`、`*tests.rs` 和精确 `#[cfg(test)]` 所保护的项，包括外部测试模块；跳过该项后恢复扫描，不丢弃同文件后续生产代码。注释和字面量不计数，导入、完整路径、别名声明和宏输入中的类型标识符计数。不展开宏或执行名称解析，不追踪别名使用、glob 导入和推断类型；因此围栏不是所有直接字段读写的静态证明，F1/F2 仍须迁移这些路径并封装内部可变状态。

## 运行时范围

永久允许范围固定在测试中，不由业务基线扩展：

| 范围 | 归属理由 |
| --- | --- |
| `application/run_access.rs`、`application/run_access/` | Run 访问与兼容序列化的归属边界；当前仅实施首个读取切片 |
| `execution.rs` | 共享 Runner 的执行装配与调用 |
| `main.rs` | 宿主、CLI 与 Runtime 的组合根 |
| `node_host.rs`、`node_host/` | NodeExecutionPort 的实现与装配；根文件和同名目录共同形成一个运行时模块，不另豁免业务路径 |
| `run_data.rs` | Runtime 产物及历史数据访问 |

`anchor-graph-host` 整个 crate 属于运行时侧，不在本围栏范围内。其余 Host 文件默认为业务侧，包括当前没有类型标识符、但仍通过推断类型读写 Run 的文件。

## HTTP 字节基线

[新增 HTTP 回归](../rust/anchor-runner-host/src/api/tests/run_access_json.rs) 固定 Run 身份与时间，逐字节比较 `/runs` 的原始响应，不先解析或重新序列化 JSON。覆盖十种既有状态及其非活动投影，包含控制动作、终态、静止、注意提示和字段顺序。等待恢复的样本保留有效 cursor 与待处理 attempt。

当前序列化保留字段插入顺序。F1 不得仅验证 JSON 对象等价，或在将状态序列化移入门面时调整字段次序。新增基线不替代后续详情、Pilot、活动执行、并发控制和恢复路径的原有断言与专项核查。

[Pilot 字节回归](../rust/anchor-runner-host/src/application/run_access/tests.rs) 固定列表与控制投影的原始 JSON 字节，详情的 record 部分按重构前直接序列化 record 的字段顺序比较，不以解析后的对象等价代替字节比较。当前仅固定 Ready 样本，十种状态的 HTTP 基线继续使用 F0 原断言。

## 规则度量：F1 首个切片

迁移前固定为 `e3a5778`。统计独立判断体，不统计 helper 调用次数、类型引用次数或状态赋值次数；既有 `RunStatus::is_terminal` 调用不算重复实现。下面第一张表覆盖本切片迁入的规则，1→1 表示统一了归属，并不表示减少了一份原本不存在的重复规则。最终验收仍须补齐全 Host 表。

| 谓词/规则 | master 原语义与来源（`src/` 相对路径） | 迁移前 → 当前独立实现数 |
| --- | --- | --- |
| `RunView::phase` | Ready/Running→executing；Paused/Stopped/BudgetStopped→waiting human；WaitingCall→waiting child；WaitingRecovery→waiting effect；其余 settled | 0 → 1（新增内部视图映射，不改 wire） |
| `is_settled` | 仅 Completed/Failed/Aborted；门面委托既有 Runtime `is_terminal` | 1 → 1（Runtime 权威集合保留） |
| `is_quiescent` | 无执行登记且终态或 Stopped；Paused/BudgetStopped 不算静止；`application/liveness.rs:32` | 1 → 1 |
| `needs_attention` | 既有投影 phase 为 failed/aborted/waiting_resume/waiting_recovery/budget_stopped；活动 stop/pause 的临时 phase 不提示；`application/run_projection.rs:89` | 1 → 1 |
| `can_pause` | active、非终态、非 Session 调用、无待处理控制；`application/run_projection.rs:99` | 1 → 1 |
| `can_stop` | active 时尚未请求 stop；inactive 时复用状态校验；`application/run_projection.rs:100` | 1 → 1 |
| `can_resume`（观察） | inactive、非 Session 调用、预算校验通过，且普通校验通过或有待恢复 attempt；`application/run_projection.rs:83`。身份、助手、放弃意图、后继等额外门槛仍由同一门面投影按原次序补查 | 1 → 1 |
| `validate_inactive_control` | 依次检查 WaitingRecovery 的非 stop、终态、inactive pause；保留原错误及顺序；`application/control.rs:229` | 1 → 1 |
| `validate_resume_budget` | BudgetStopped 必须显式预算策略；`application/control.rs:245` | 1 → 1 |
| `validate_control_identity` | metadata 与冻结 record 的 Graph digest 相同；`application/control.rs:208` | 1 → 1 |
| `can_abandon`（入口许可） | Completed/Failed 不允许，其余允许；`application/abandon.rs:204`。原入口的 Aborted 幂等响应仍保留，冲突许可已调用门面，收口时的并发结果分类尚未迁移 | 1 → 1（仅入口许可，不含最终结果分类） |
| `legacy_status` | API 原十种状态词汇；`api.rs:227`，Pilot 的 serde 状态词汇相同 | 显式映射 1 → 1（生成点移至门面） |

### 已核实但未收敛的重复判断

下表范围为任务指定的 `application/{control,admission,abandon,conversations,dispatch,session_calls,deletion,graphs,recovery}.rs` 与 `api/{wecom,wecom_progress,turns,files}.rs`，不是全 Host 最终审计。`T` 表示 Completed/Failed/Aborted；所有行号指迁移前提交。

| 待提取规则 | 原位置 | 迁移前 → 当前独立实现数 |
| --- | --- | --- |
| `has_stopped_execution`：状态为 `T ∪ {Stopped}` 或其补集，不含执行登记 | `application/control.rs:19`、`application/session_calls.rs:103`、`api/wecom.rs:858`、`api/turns.rs:33`、`api/turns.rs:51` | 5 → 2（剩 Session 判断） |
| `can_dispatch`：detached Run 仅 Ready 可派遣/恢复 | `application/dispatch.rs:51`、`application/dispatch.rs:78` | 2 → 1 |
| wait 父 Run 的 WaitingCall 与 cursor node/invocation 共同匹配 | `application/control.rs:71`、`application/dispatch.rs:172` | 2 → 2 |
| `waits_for_children`：stop/abandon 时是否清理等待子 Run | `application/control.rs:123`、`application/control.rs:132`、`application/abandon.rs:281` | 3 → 2（剩 abandon 判断） |
| 成功状态必须 Completed，不等于完整投递许可 | `application/conversations.rs:171`、`application/dispatch.rs:149`、`application/session_calls.rs:425`、`api/files.rs:196` | 4 → 4 |
| 恢复目标的 cursor、attempt 与 WaitingRecovery 共同条件 | `application/recovery.rs:104`、`application/recovery.rs:206` | 2 → 2 |

后续不可把相同子条件误当成完整规则相同：Session 投递先检查 active，文件读取先判断 Completed；普通渠道与常驻助手结算对 Completed 的处理不同；恢复前置验证与租约内复核的错误顺序不同；Graph 级联删除与单 Run 删除、等待闭包交接的缺失记录处理也不同。这些差别必须封在按意图命名的谓词/校验中，不改变原短路与错误顺序。

## 尚未关闭

- F1 首个切片的定向回归、Rust workspace（1,959 通过、122 忽略）、fmt/clippy、Web 306 测试及构建通过；本轮没有执行 Goose 忽略套件，不宣称完整 F1 验收或相对 Goose 基线无新增失败。
- F0 已按任务书审查通过；固定 Goose 的 ACP 扩展初始化失败由 G0 独立处理。G0 关闭前按任务书以失败名单为基线，不将本轮未执行的套件写成通过。F0 原始验证与 F1 首个切片的定向证据见[本机验证](../.local/run-access-facade-20261019/README.md)，原始日志仅在本机保有。
- 第二个切片新增 HTTP 详情十状态与 abandon 原始 JSON 字节回归，以及渠道和助手 binding 的边界回归；验证结果见[第二切片证据](../.local/run-access-facade-20261019/f1-readers/README.md)。F1 仍需补齐 `RunView` 契约，迁完所有只读消费者与兼容序列化，并将逐规则表扩展到全 Host；本页当前表明确保留未收敛数，不以围栏标识符数替代该度量。
- F2 仍需按意图封装 Run 写入；不得用类型重命名、放宽运行时豁免或直接暴露原始 record 来降低围栏数。迁移前继续沿用现有事实所有者、租约、执行登记及持久化顺序。
