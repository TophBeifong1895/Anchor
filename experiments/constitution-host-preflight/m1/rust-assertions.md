# M1 Rust 断言登记

范围：相对 `d915314`。按既有期望表达式或轮询目标计数，共 **26 处**（20 处断言期望、6 处轮询目标）；参数展开单列，不与表达式数混算。新增断言与新增测试另列。产物、成本、路由、授权拒绝、不重放、Session 投递断言均保留；删除测试 0、新增 ignore 0。

| ID | 文件 / 测试或 fixture | 旧期望 → 新期望 | 依据 |
| --- | --- | --- | --- |
| MR01 | api/tests.rs / paused_wait_child_resumes_independently_then_releases_parent / 轮询 | child.status Stopped → Paused | M1 纠正 S25，恢复边界暂停 |
| MR02 | 同上 / 结果断言 | child.status Stopped → Paused | 同上；恢复和父子释放断言保留 |
| MR03 | api/tests.rs / start_and_pause_request / 响应 operation | stop → pause | M1 独立控制动作 |
| MR04 | 同上 / 响应 alias | alias=true → 不含 alias | M1 取消别名协议 |
| MR05 | 同上 / 存储状态 | Stopped → Paused | M1；原 results.len()==1 不变 |
| MR06 | api/tests.rs / waiting_parent_rejects_child_grant_drift_then_restores_same_child_without_replay | child_before.status Stopped → Paused | M1；授权拒绝、产物、首结果与不重放断言不变 |
| MR07 | api/tests/conversations.rs / conversation_resume_is_not_blocked_by_another_session_on_the_same_graph / 轮询 | Bob Stopped → Paused | M1；下一轮仍 409、原结果与调用次数不变 |
| MR08 | 同文件 / superseded_conversation_cannot_resume_or_recover_its_wait_child / 轮询 | 暂停后 child Stopped → Paused | M1；随后显式 stop 仍 Stopped，旧轮恢复拒绝不变 |
| MR09 | 同文件 / superseded_conversation_keeps_detached_child_independent / 轮询 | 暂停后 detached child Stopped → Paused | M1；随后显式 stop 及 detach 边界不变 |
| MR10 | api/tests/kernel_probe.rs / kernel_read_view_http_tracks_running_human_wait_and_settlement_with_host_time / Executing actions | stop/resume/abandon 原值保留，增加 pause=true | M1；有 worker 的 Executing 可暂停 |
| MR11 | 同上 / Waiting actions | 原三动作保留，增加 pause=false | M1；等待时不可暂停 |
| MR12 | 同上 / Settled actions | 原三动作保留，增加 pause=false | M1；结算时不可暂停 |
| MR13 | application/legacy_run_view.rs / every_legacy_status_maps_lossily_with_both_worker_observations | actions 无 pause → pause==(executing && worker) | M1；10 状态 × 2 worker，共 20 个参数实例 |
| MR14 | application/run_projection.rs / legacy_projection_serialization_preserves_ready_paused_and_stopped_bytes / Ready | 精确字节 JSON actions 增加 pause:false | M1；其余字段字节不变 |
| MR15 | 同上 / Paused | 精确字节 JSON actions 增加 pause:false | 同上 |
| MR16 | 同上 / Stopped | 精确字节 JSON actions 增加 pause:false | 同上 |
| MR17 | application/run_view.rs / actions_cover_every_phase_waiter_and_outcome_with_and_without_worker | AvailableActions 增加 pause==(Executing && worker) | M1；9 相位/waiter/outcome × 2 worker，18 个参数实例 |
| MR18 | api/runs/tests.rs / list_and_detail_share_status_control_and_attention_projection / phase 参数期望 | ready/waiting_call/waiting_resume/waiting_recovery/budget_stopped/completed → executing/waiting/executing/waiting/waiting/settled | S19/S20；六实例逐项见下表 |
| MR19 | 同上 / resume 参数期望 | WaitingCall、WaitingRecovery true→false；BudgetStopped false→true，其余不变 | S23；只改动作投影期望，旧写入守卫未动 |
| MR20 | 同上 / stop 参数期望 | WaitingCall、WaitingRecovery、BudgetStopped true→false，其余不变 | S22；Executing 才可 stop |
| MR21 | 同上 / attention 断言 | projection.needs_attention → waiting 或 executing+inactive 的事实推导；Ready/WaitingCall false→true | S19/S20；同时断言旧字段不存在，六实例见下表 |
| MR22 | 同文件 / incomplete_run_identity_does_not_offer_inactive_controls / detail | 三个动作 false → 四个动作 false，增加 abandon=false | S22；身份拒绝保持全部动作不可用 |
| MR23 | 同上 / list | 三个动作 false → 四个动作 false，增加 abandon=false | 同上；不放宽授权拒绝 |
| MR24 | tests/standard_host.rs / standard_host_runs_op_without_goose_model_or_runtime_switch / 失败断言 | state.status!=failed → projection.outcome!=failed | S19；仍拒绝失败结局，原产物断言不变 |
| MR25 | 同上 / 完成轮询 | state.status=completed → projection.phase=settled && outcome=done | S19；必须成功结算，不接受失败/等待替代 |
| MR26 | 同文件 / old_state_root_still_allows_op_without_becoming_goose_recovery / 完成轮询 | state.status=completed → projection.phase=settled && outcome=done | S19；旧事实与 Goose 目录不变断言保留 |

MR18–MR21 参数实例（括号为 resume / stop / attention）：

| 旧 Run 状态 | 旧 phase 与动作/attention | 新 phase / waiter / outcome 与动作/attention |
| --- | --- | --- |
| Ready | ready (true / true / false) | executing / — / — (true / true / true) |
| WaitingCall | waiting_call (true / true / false) | waiting / child / — (false / false / true) |
| Running | waiting_resume (true / true / true) | executing / — / — (true / true / true) |
| WaitingRecovery | waiting_recovery (true / true / true) | waiting / effect / — (false / false / true) |
| BudgetStopped | budget_stopped (false / true / true) | waiting / human / — (true / false / true) |
| Completed | completed (false / false / false) | settled / — / done (false / false / false) |

六实例均新增 waiter/outcome、pause=false 和 needs_attention 缺席检查。列表与详情完整 projection 相等断言保留。

新增到既有测试的检查：legacy 映射检查 pause 规则；inactive projection 检查 10 个状态 pause=false；pending-control 检查 pause=true 及 Session call 收窄为 false。原 stop/resume/abandon、静止、时间和其他协议断言不变。

新增测试 8 项：

- `application::kernel_runs::tests::pause_commits_current_result_before_human_wait_and_resume_does_not_replay`
- `application::kernel_runs::tests::pause_on_final_node_preserves_settlement_and_all_committed_results`
- `application::kernel_runs::tests::stop_preempts_pending_pause_without_repausing_the_resumed_run`
- `application::kernel_runs::tests::pending_pause_survives_host_restart_without_dispatching_another_node`
- `application::kernel_runs::tests::pending_pause_restart_commits_durable_completion_before_waiting`
- `api::tests::kernel_probe::legacy_and_kernel_http_share_executing_paused_and_settled_projections`
- `pilot_tools::tests::run_pause_requests_the_node_boundary_without_cancelling_the_worker`
- `pilot_tools::tests::run_stop_cancels_the_worker_without_requesting_a_boundary_pause`

实际执行见 [workspace.log](workspace.log)。Web 的 4 处既有表达式、fixture schema 增补与 5 项新增测试见 [Web 登记](web-assertions.md)。R2 的历史登记保留；本表明确撤销其暂停别名相关期望，不改写历史证据。
