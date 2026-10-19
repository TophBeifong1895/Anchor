# R2 Rust 测试断言修改登记

审计基准：`b1e9bc1503766c33a2ee0c87ece0ec93beae4716`；范围：`git diff b1e9bc1 74a4dec -- rust`。本表描述源码 `74a4dec` 的未完成候选实现，**不表示测试通过或 S 候选获批**。

按动态表逐状态、worker、断言分量展开：修改既有断言 **84 条**，另改轮询/等待目标 **6 条**；既有 fixture 新增别名断言 **2 条**。新增映射测试 **1 个**，单列于文末，不计入既有断言修改。相同 assert 宏被不同表行触发时逐行登记，不能把上述数字当成源码 assert 宏数量。

S 编号见工作记录的 R2 候选表；以下分组逐项关联同一编号。可机读逐项记录见 `rust-assertions.json`。

## 审计结论与边界

- 未发现既有非状态断言被删除、改值或改成宽松比较。HTTP 返回码、文件/产物、执行次数、冻结输入、身份、元数据和时间戳比较均保留。尤其 `start_and_pause_request` 的 `record.results.len() == 1`、两个 busy 测试的拒绝/不建 Run 断言均未修改；pause→stop 别名使它们受影响，是保留的失败证据，不能为了通过而改断言。
- `control_requested` 被新 schema 删除：application 中 5 处、session_calls helper 中 1 处、platform_contract 中 2 处原断言已替换为 phase 检查。这些替换仍检查状态，但**不再验证 pending token 是否暴露及 stop 对 pause 的优先级**。必须按行为变化审查，不能描述为语义等价。
- `run_projection` 的 `needs_attention`、`quiescent` 字段被删除，测试分别改为新字段推导式和 `RunLiveness`；逐行旧值/新值如下。Ready 的注意项从 false 变为 true；这是实际变化。
- 字节序列测试仍精确比较完整 JSON；为新增的可空时间字段显式设置 `record.updated=None`。原测试没有时间字段检查；现有持久化时间测试的非空值、mtime 精确比较没有削弱。
- 未发现删除测试或新增 `#[ignore]`。唯一新增 Rust 测试为全 10 旧状态 × 2 worker 的映射测试。

## S 候选分组与受影响对象

| 分组 | 旧行为 → 新行为 / 受影响对象 |
| --- | --- |
| S19：新 phase/outcome 词汇 | API/Pilot/Web 的旧状态及 terminal 标志 → phase/waiting_for/outcome；消费旧字段的客户端受影响。 |
| S20：无 worker 的 Executing 可以 resume | 共享 RunView 的 Executing 无 worker 动作由 resume=false → true（原 Kernel JSON 已另行允许启动 driver）；重启后的驱动操作受影响。 |
| S21：Paused/BudgetStopped 统一为静止 | 无 worker 的 Paused/BudgetStopped 不静止 → 静止；会话准入、后继和等待闭包消费者受影响。 |
| S22/S24：动作按相位推导、移除 pending-control 展示 | 旧状态/请求旗标/旧 guard 的展示分支 → 相位动作和新展示；pending 控制、WaitingChild、WaitingEffect 等动作可见性受影响。 |
| S25：pause 别名即时 stop | pause 在节点边界暂停 → 调用 stop；正在执行的节点会被取消，原来依赖暂停提交/占用的调用者和测试受影响。 |
| S23：取消旧恢复/预算 resume 展示限制 | 旧预算 guard 抑制 resume、effect recovery 特例 → human 无 worker 可 resume、effect 不可 resume；实际写入 guard 保持原样。 |
| S26：updated 可空 | 对外新增统一 updated，可空；HTTP 时间消费者必须处理 null，原持久化 mtime 行为保留。 |

## 逐项既有断言、轮询与新增别名检查

### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`graph_crud_round_trips_authoring_modules_and_layout`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA001 / 修改轮询 | 等待执行结束的轮询条件 | `HTTP OK && state.status != running && state.status != ready` | `HTTP OK && projection.phase != executing` | S19：新 phase/outcome 词汇 |
| RA002 / 修改断言 | 完成结局 | `detail[state][status] == completed` | `detail[projection][outcome] == done` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`trigger_persists_run_and_projects_list_detail_and_control`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA003 / 修改轮询 | 等待执行结束的轮询条件 | `HTTP OK && state.status != running && state.status != ready` | `HTTP OK && projection.phase != executing` | S19：新 phase/outcome 词汇 |
| RA004 / 修改断言 | 完成结局 | `detail[state][status] == completed` | `detail[projection][outcome] == done` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`paused_wait_child_resumes_independently_then_releases_parent`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA005 / 修改轮询 | 子 Run 停下的轮询条件 | `store child.status == RunStatus::Paused` | `store child.status == RunStatus::Stopped` | S25：pause 别名即时 stop |
| RA006 / 修改断言 | 子 Run 的存储状态 | `store child.status == RunStatus::Paused` | `store child.status == RunStatus::Stopped` | S25：pause 别名即时 stop |

### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`start_and_pause_request (fixture)`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA007 / 修改断言 | fixture 最终存储状态 | `record.status == RunStatus::Paused` | `record.status == RunStatus::Stopped` | S25：pause 别名即时 stop |
| RA008 / 新增断言 | 新增：别名的有效操作 | `无该断言` | `alias[operation] == stop` | S25：pause 别名即时 stop |
| RA009 / 新增断言 | 新增：响应说明别名 | `无该断言` | `alias[alias] == true` | S25：pause 别名即时 stop |

### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`downstream_failure_does_not_reclassify_committed_upstream_result`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA010 / 修改断言 | 失败结局 | `detail[state][status] == failed` | `detail[projection][outcome] == failed` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests.rs`

测试 / fixture：`waiting_parent_rejects_child_grant_drift_then_restores_same_child_without_replay`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA011 / 修改断言 | 停下后的子 Run 存储状态 | `child_before.status == RunStatus::Paused` | `child_before.status == RunStatus::Stopped` | S25：pause 别名即时 stop |

### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`conversation_resume_is_not_blocked_by_another_session_on_the_same_graph`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA012 / 修改轮询 | bob_run 的 wait_run_status 目标 | `RunStatus::Paused` | `RunStatus::Stopped` | S25：pause 别名即时 stop |

### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`superseded_conversation_cannot_resume_or_recover_its_wait_child`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA013 / 修改轮询 | child_run 的 wait_run_status 目标 | `RunStatus::Paused` | `RunStatus::Stopped` | S25：pause 别名即时 stop |

### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`superseded_conversation_keeps_detached_child_independent`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA014 / 修改轮询 | detached child_run 的 wait_run_status 目标 | `RunStatus::Paused` | `RunStatus::Stopped` | S25：pause 别名即时 stop |

### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`superseded_conversation_cannot_resume_or_recover_its_wait_child`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA015 / 修改断言 | 子 Run 对外相位 | `detail[state][status] == stopped` | `detail[projection][phase] == waiting` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/conversations.rs`

测试 / fixture：`conversation_orphan_running_requires_explicit_stop_before_a_new_turn`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA016 / 修改断言 | 无 worker 的 Running 对外相位 | `detail[state][status] == running` | `detail[projection][phase] == executing` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/kernel_probe.rs`

测试 / fixture：`explicit_kernel_probe_runs_real_host_ops_and_projects_existing_queries`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA017 / 修改断言 | 详情相位 | `detail[state][status] == completed` | `detail[projection][phase] == settled` | S19：新 phase/outcome 词汇 |
| RA018 / 修改断言 | 详情结局 | `detail[projection][terminal] == true` | `detail[projection][outcome] == done` | S19：新 phase/outcome 词汇 |
| RA019 / 修改断言 | 列表结局 | `list[runs][0][status] == completed` | `list[runs][0][projection][outcome] == done` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/kernel_probe.rs`

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

### `rust/anchor-runner-host/src/api/tests/platform_contract.rs`

测试 / fixture：`trigger_freezes_objective_and_manual_or_schedule_source`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA027 / 修改断言 | 去掉旧 pending-control 字段后的状态断言 | `detail[control_requested] == null` | `detail[projection][phase] == settled` | S22/S24：动作按相位推导、移除 pending-control 展示 |

### `rust/anchor-runner-host/src/api/tests/platform_contract.rs`

测试 / fixture：`updated_tracks_persisted_pause_stop_and_resume_after_restart`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA028 / 修改断言 | pause 别名后的相位 | `paused[state][status] == paused` | `paused[projection][phase] == waiting` | S19：新 phase/outcome 词汇；S25：pause 别名即时 stop |
| RA029 / 修改断言 | 重启后状态（原查 pending-control） | `reopened[control_requested] == null` | `reopened[projection][phase] == waiting` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA030 / 修改断言 | stop 后相位 | `stopped[state][status] == stopped` | `stopped[projection][phase] == waiting` | S19：新 phase/outcome 词汇 |
| RA031 / 修改断言 | resume 完成后相位 | `completed[state][status] == completed` | `completed[projection][phase] == settled` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/run_abandon.rs`

测试 / fixture：`abandon_cancels_the_running_node_and_makes_the_run_terminal_aborted`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA032 / 修改断言 | abandon 响应结局 | `body[status] == aborted` | `body[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/run_abandon.rs`

测试 / fixture：`abandon_is_idempotent_and_rejects_terminal_unknown_or_invalid_requests`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA033 / 修改断言 | 默认 reason 的响应结局 | `defaulted[status] == aborted` | `defaulted[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |
| RA034 / 修改断言 | 已 aborted 的响应结局 | `existing[status] == aborted` | `existing[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/run_abandon.rs`

测试 / fixture：`abandon_keeps_the_assistant_binding_workspace_and_history_for_handover`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA035 / 修改断言 | 交接前 abandon 响应结局 | `abandoned[status] == aborted` | `abandoned[projection][outcome] == aborted` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/session_calls.rs`

测试 / fixture：`completed_session_execution_stays_waiting_until_delivery_settlement`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA036 / 修改断言 | 执行完成且 delivery pending 的相位 | `detail[state][status] == completed` | `detail[projection][phase] == settled` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/api/tests/session_calls.rs`

测试 / fixture：`completed_pending_wait_child_recovers_delivery_and_parent_after_restart`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA037 / 修改断言 | 子 Run 列表结局 | `child_projection[status] == completed` | `child_projection[projection][outcome] == done` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/application/kernel_runs/tests.rs`

测试 / fixture：`serial_probe_commits_artifacts_cost_and_phase_through_application`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA038 / 修改断言 | 串行 probe 完成 | `assert!(project_run(...).terminal)` | `assert!(project_run(...).phase == settled)` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/application/kernel_runs/tests.rs`

测试 / fixture：`abandon_waiting_probe_is_terminal_and_refuses_late_submission`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA039 / 修改断言 | 等待 probe 被放弃 | `assert!(project_run(...).terminal)` | `assert!(project_run(...).phase == settled)` | S19：新 phase/outcome 词汇 |

### `rust/anchor-runner-host/src/application/liveness.rs`

测试 / fixture：`parked_or_unregistered_ready_runs_are_not_quiescent`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA040 / 修改断言 | RunStatus::Paused、worker=false | `RunLiveness::observed(status,false).is_quiescent() == false` | `RunLiveness::observed(status,false).is_quiescent() == true` | S21：Paused/BudgetStopped 统一为静止 |
| RA041 / 修改断言 | RunStatus::BudgetStopped、worker=false | `RunLiveness::observed(status,false).is_quiescent() == false` | `RunLiveness::observed(status,false).is_quiescent() == true` | S21：Paused/BudgetStopped 统一为静止 |

### `rust/anchor-runner-host/src/application/run_view.rs`

测试 / fixture：`actions_cover_every_phase_waiter_and_outcome_with_and_without_worker`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA042 / 修改断言 | Phase::Executing、worker=false；AvailableActions.resume 分量 | `AvailableActions {stop:true,resume:false,abandon:true}` | `AvailableActions {stop:true,resume:true,abandon:true}` | S20：无 worker 的 Executing 可以 resume |

### `rust/anchor-runner-host/src/application/run_projection.rs`

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

### `rust/anchor-runner-host/src/application/run_projection.rs`

测试 / fixture：`pending_control_and_session_coordination_are_observed_once`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA080 / 修改断言 | active=true、requested=pause 的相位 | `projection.phase == pausing` | `projection.phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA081 / 修改断言 | active=true、requested=pause 的动作字段 | `assert!(!projection.actions.pause)` | `assert!(projection.actions.abandon)` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA082 / 修改断言 | active=true、requested=stop 的相位 | `projection.phase == stopping` | `projection.phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA083 / 修改断言 | active=true、requested=stop 的动作可见性 | `assert!(!projection.actions.stop)` | `assert!(projection.actions.stop)` | S22/S24：动作按相位推导、移除 pending-control 展示 |

### `rust/anchor-runner-host/src/application/run_projection.rs`

测试 / fixture：`legacy_projection_serialization_preserves_ready_paused_and_stopped_bytes`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA084 / 修改断言 | RunStatus::Ready：serde_json::to_vec(...) == expected.as_bytes() | `{"phase":"ready","active":false,"control_requested":null,"terminal":false,"quiescent":false,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}` | `{"phase":"executing","reason":null,"active":false,"actions":{"stop":true,"resume":true,"abandon":true},"updated":null}` | S19：新 phase/outcome 词汇；S22/S24：动作按相位推导、移除 pending-control 展示；S26：updated 可空 |
| RA085 / 修改断言 | RunStatus::Paused：serde_json::to_vec(...) == expected.as_bytes() | `{"phase":"paused","active":false,"control_requested":null,"terminal":false,"quiescent":false,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}` | `{"phase":"waiting","waiting_for":"human","reason":"paused","active":false,"actions":{"stop":false,"resume":true,"abandon":true},"updated":null}` | S19：新 phase/outcome 词汇；S22/S24：动作按相位推导、移除 pending-control 展示；S26：updated 可空 |
| RA086 / 修改断言 | RunStatus::Stopped：serde_json::to_vec(...) == expected.as_bytes() | `{"phase":"stopped","active":false,"control_requested":null,"terminal":false,"quiescent":true,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}` | `{"phase":"waiting","waiting_for":"human","reason":"stopped","active":false,"actions":{"stop":false,"resume":true,"abandon":true},"updated":null}` | S19：新 phase/outcome 词汇；S22/S24：动作按相位推导、移除 pending-control 展示；S26：updated 可空 |

### `rust/anchor-runner-host/src/application/tests.rs`

测试 / fixture：`pending_control_projects_active_tokens_with_stop_priority`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA087 / 修改断言 | 缺少 worker 的 missing record | `project_run(...).control_requested == None` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA088 / 修改断言 | 已登记 worker、未请求控制 | `project_run(...).control_requested == None` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA089 / 修改断言 | pause flag=true | `project_run(...).control_requested == Some(pause)` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA090 / 修改断言 | pause+cancellation flags=true；原 stop 优先 | `project_run(...).control_requested == Some(stop)` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |
| RA091 / 修改断言 | 移除 worker 后 | `project_run(...).control_requested == None` | `project_run(...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |

### `rust/anchor-runner-host/src/application/session_calls.rs`

测试 / fixture：`stopped_parent_cancels_wait_delivery (fixture) → active_parent_stop_cancels_wait_session_delivery_but_not_detach`。

| ID / 类型 | 情形 | 旧断言或目标 | 新断言或目标 | S 候选分组 |
| --- | --- | --- | --- | --- |
| RA092 / 修改断言 | active_parent=true，stop 后仍排空的父 Run | `project_run(parent,...).control_requested == Some(stop)` | `project_run(parent,...).phase == executing` | S22/S24：动作按相位推导、移除 pending-control 展示 |

## fixture 关联测试

`start_and_pause_request` 直接或经 `start_and_pause` 影响以下既有测试；本表中的 fixture 断言只计一次，未重复算到每个调用者：

- `api/tests.rs::pause_resume_uses_same_frozen_run_after_graph_edit_and_service_object_restart`
- `api/tests/platform_contract.rs::busy_schedule_creates_no_run_or_source_metadata`
- `api/tests/platform_contract.rs::updated_tracks_persisted_pause_stop_and_resume_after_restart`
- `api/tests/schedules.rs::busy_due_schedule_skips_existing_admission_without_creating_a_run`
- `api/tests/run_abandon.rs::abandon_leaves_other_runs_and_their_recovery_entry_points_unchanged`

`application/session_calls.rs::stopped_parent_cancels_wait_delivery(true)` 的改动断言由 `active_parent_stop_cancels_wait_session_delivery_but_not_detach` 执行；false 分支的存储状态断言未改，关联测试为 `waiting_parent_stop_cancels_completed_pending_session_child_without_waking_parent`。

## 动态表完整行变化核对

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

## 新增测试（不混计既有断言修改）

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

## 可复核范围

本次提交前审计快照的 Rust diff SHA-256：`bad1ab66a881999d9afb138a638b39ea255550df73538d6d52a0df15c09b57a3`。该值只标识审计快照；后续生产时间观察小改不会自动改变本表已登记的测试断言。可用 `git diff b1e9bc1 74a4dec -- rust` 重新核对；测试是否通过以主报告的实际日志为准。
