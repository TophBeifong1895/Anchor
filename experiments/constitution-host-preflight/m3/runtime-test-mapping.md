# Runtime 删除切片与测试去向

旧执行器、旧生命周期类型和旧整体记录存储已删除。产品执行只通过 KernelScheduler；测试 Harness 仅连接真实 Kernel、Store 和 Ports。73 个原 main 测试名称全部保留，无新增 ignore；Host operations 与 invocation authority 的去向见同目录的总验收索引。

GraphRunRecord format 为 14；格式 1–13 和旧 status/cursor/environments 字段具名拒绝。历史快照使用独立 FrozenOldRecord wire，生产 GraphRunRecord 不读取旧生命周期。快照中的旧 cursor 只用于既有 S6/S18 对照校验，不参与运行。

GraphValidator 生产改动只有删除 branch 构造中的 cursor 字段。结构一致性校验在 scheduler.load 与 Host 读取边界执行，record_of 保持纯 decode。GraphRecord.validate 接受现有 module ceiling 已产生的退役 ingress：false、零source/result引用、新序列且所属scope存在ceased上限；这不修改 frontier、prepare 或裁决。

S6、S18 与其他已批准差异保持原差分规则。预算/中断未完成 invocation 不提前写 passes，取消仅结束 worker；已观察的非法route、授权失败及并行Yielded通过既有 Code Settle 终止，GraphValidator 不变。

| 原测试（函数名保留） | 当前文件 | 迁移后断言与旧机制去向 |
| --- | --- | --- |
| `only_completed_failed_and_aborted_runs_are_terminal` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L2) | Kernel Phase/Outcome 的实际 terminal 判定；旧 Ready/Running、暂停/预算等标签改由 Executing/Waiting 事实表达。 |
| `host_assigned_run_identity_is_preserved_and_validated` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L26) | 纯Graph/快照事实测试原业务断言不变。 |
| `executed_nodes_follow_first_durable_completion_order` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L39) | 纯Graph/快照事实测试原业务断言不变。 |
| `waiting_recovery_preserves_cursor_and_direct_graph_resume_cannot_bypass_it` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L69) | Code 观察写入 Human waiter 与 history attempts；frontier 派生同 key，重复 scheduler.run 不释放等待、不重复派发。 |
| `graph_call_wait_resumes_same_identity_and_detach_records_acceptance` | [migrated_calls.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_calls.rs#L2) | 实际 Call Bound/Child waiter/Completed；同 drive 的 child observation 增加一次同身份查询，所有身份仍相同。detach 输出仅 accepted reference。 |
| `graph_call_identity_must_keep_one_child_run_across_reload` | [migrated_calls.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_calls.rs#L111) | 实际 Bound 后再次观察，child id 改变仍具名失败。 |
| `stopped_conversation_run_can_retain_unknown_recovery_attempts` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L101) | 保留工具观察；实际 Human stop 是 lifecycle 权威，Graph 观察数组不决定 phase。 |
| `plugin_authorization_drift_fails_before_dispatch` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L2) | 构造时绑定基线；预算观察后显式 Human resume，授权变化 Code Settle Failed，零额外 node calls。 |
| `plugin_code_changes_resume_and_are_observed_per_call` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L55) | 当前代码资源与 immutable admitted authority 分开；同 invocation 的 view/digest 观察保留原断言。 |
| `parallel_dispatch_uses_current_plugin_resources_and_preserves_admitted_authority` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L116) | 生产 scheduler 并行路径；当前资源、原准入基线与各结果观察断言均保留。 |
| `legacy_plugin_authority_requires_offline_migration_before_execution` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L246) | 缺失/null/空 authorization decoder 具名拒绝；实际 Kernel FileStore opaque bytes 不改写，不派发 node。 |
| `a_new_run_does_not_bind_plugins_without_a_current_authorization_baseline` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L310) | create_with_plugins 拒绝空代码digest；无基线派发 Code Settle，仍无binding、零node calls；AuthorizationDigest 类型断言保留。 |
| `a_node_call_records_the_dependency_view_it_used` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L177) | 生产 scheduler 写入 result plugin_views，并验证读取消费观察。 |
| `legacy_in_progress_plugin_run_without_binding_fails_closed` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L212) | 无 binding baseline 在派发前 Code Settle；旧独立 Paused 标签消失，零 node calls 不变。 |
| `serial_routing_skips_unselected_and_passes_fixed_commit_input` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L2) | 纯Graph/快照事实测试原业务断言不变。 |
| `skipped_diamond_branch_propagates_false_edge_and_releases_join` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L53) | 纯Graph/快照事实测试原业务断言不变。 |
| `skipped_multi_level_chain_propagates_to_join_without_running_nodes` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L93) | 纯Graph/快照事实测试原业务断言不变。 |
| `inactive_unstarted_cycle_closes_and_releases_selected_merge` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L139) | 纯Graph/快照事实测试原业务断言不变。 |
| `inactive_scc_waits_for_external_incoming_decision` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L83) | 纯Graph/快照事实测试原业务断言不变。 |
| `false_ingress_closes_cascaded_sccs_deterministically` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L117) | 纯Graph/快照事实测试原业务断言不变。 |
| `skipped_propagation_does_not_invent_decisions_inside_unentered_cycle` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L251) | 纯Graph/快照事实测试原业务断言不变。 |
| `later_false_loop_input_retires_stale_selected_output` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L283) | 纯Graph/快照事实测试原业务断言不变。 |
| `op_run_policy_and_command_are_passed_to_the_host_port` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L183) | 纯Graph/快照事实测试原业务断言不变。 |
| `budget_stop_keeps_cursor_and_resume_uses_same_invocation` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L130) | S6：未完成 invocation/passes 为零；frontier/key不变，成本2入history，Human resume后同key完成仅计一次。 |
| `serial_file_store_reload_resumes_same_invocation_after_interruption` | [migrated_reload.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_reload.rs#L2) | 实际 Kernel FileStore reopen，同 frontier key；显式 Human resume，同 key 两次调用、一次完成。 |
| `serial_interrupted_file_store_reload_resumes_same_invocation_and_clears_error` | [migrated_reload.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_reload.rs#L47) | 实际 FileStore reopen；保留前驱结果、同key/输入/commits重试，后继一次；S6未完成不计数，错误理由归Human waiter，完成无Graph error。 |
| `graph_runner_dispatches_durable_resumable_node_with_same_invocation` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L176) | CompletionFact::Resumable 由生产 scheduler 以原key派发一次，完成计数一次。 |
| `known_node_failure_is_terminal_and_does_not_advance_edges` | [migrated_storage.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_storage.rs#L54) | 旧 RunStore save#3 故障改为实际 Kernel commit#1 前故障；依durable Failed fact恢复结算，不重派发、不冻结、不推进边。 |
| `invalid_durable_route_fails_run_without_settling_or_replaying` | [migrated_storage.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_storage.rs#L99) | Kernel commit前故障后，从durable Completed重新检测同非法route，Code Settle Failed；不冻结、不写result/edge、不重派发。 |
| `completion_fact_closes_run_store_crash_window_without_reexecution` | [migrated_storage.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_storage.rs#L136) | 真实 Kernel commit前故障；durable completion及artifact重用，单次node调用与单次freeze。 |
| `durable_run_save_feedback_error_requires_reloading_before_resume` | [migrated_storage.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_storage.rs#L158) | 实际 FileStore commit后丢失反馈；持久终态优先返回 TaskSettled，stale调用不能覆盖；重新加载无重派发/重冻结。 |
| `uncertain_and_pause_fail_closed_at_invocation_boundary` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L205) | Uncertain记录Code观察Human等待；暂停是Human stop；不派发、不推进边；S6未完成不提前计数。 |
| `stop_before_dispatch_and_executor_cancel_preserve_safe_cursor` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L247) | Human stop禁止派发；Cancelled只结束本worker，Phase仍Executing，frontier同key，后续drive可完成。 |
| `run_rejects_cursor_identity_mismatch_before_dispatch` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L280) | 独立cursor不再可构造；旧cursor字段一律具名拒绝，正式dispatch身份仅派生自frontier。 |
| `admission_accepts_paired_parallel_topology_and_runner_executes_control_ops` | [migrated_plugins.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_plugins.rs#L363) | Plugin基线缺失终态失败；fanout/join由Code执行且不进node port；能力缺失仍具名Unsupported。 |
| `parallel_admission_rejects_orphan_reused_and_malformed_pairs` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L359) | 纯Graph/快照事实测试原业务断言不变。 |
| `parallel_admission_rejects_branch_forks_merges_and_cycles` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L425) | 纯Graph/快照事实测试原业务断言不变。 |
| `parallel_activation_facts_bind_pair_branch_paths_and_active_cursors` | [migrated_validation.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_validation.rs#L2) | 保留pair/path/prefix校验；旧cursor匹配校验改为旧字段具名拒绝，越界next_index仍拒绝。 |
| `parallel_branches_complete_out_of_order_and_join_receives_all_branch_facts` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L222) | 生产并行端口重叠、反序完成、join manifest完整且仅一次；原业务断言保留。 |
| `parallel_branch_failure_is_terminal_and_never_dispatches_join` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L273) | Failed事实Code Settle；原失败理由、无join、已冻结fanout与sibling观察仍保留。 |
| `parallel_resume_keeps_completed_branch_and_reuses_budget_cursor` | [migrated_reload.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_reload.rs#L294) | 确定性先完成right、延迟left预算观察；保留完整已完成分支、同key/输入重试、join一次。S6仅完成才计数。 |
| `parallel_file_store_roundtrip_preserves_activation_and_branch_cursors` | [migrated_reload.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_reload.rs#L298) | 实际FileStore reopen保留activation/prefix，frontier重建pending key/input；已完成分支不重放、join一次。 |
| `parallel_cancelled_branch_can_reload_without_replaying_completed_sibling` | [migrated_reload.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_reload.rs#L302) | 实际FileStore reopen；Cancelled不制造lifecycle写入，phase Executing；已完成分支不重放、pending同key重试。 |
| `parallel_interrupted_file_store_reload_preserves_completed_branch_and_clears_error` | [migrated_reload.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_reload.rs#L306) | 确定性right先完成、left中断；FileStore reopen后Human resume，同key/input重试且原结果不变，join/after一次。 |
| `parallel_branch_reuses_node_round_ceiling_before_dispatch` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L310) | 现有GraphValidator ceiling裁决；不派发超限left、无join、原因具名。 |
| `parallel_region_counts_module_activation_once_and_enforces_reentry_ceiling` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L344) | 生产scheduler保留首次激活一次及第二波module ceiling，原两个成员各一次和ceased断言保留。 |
| `loop_reentry_uses_fresh_selected_back_edge_and_ceiling` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L408) | 原派发序列与上限保留；旧Stopped(round limit)映射实际Settled Escalated。 |
| `module_reentry_counts_activations_and_resets_node_pass_ceiling` | [migrated_behavior.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_behavior.rs#L438) | 原激活/总invocations/每激活passes断言保留；旧Stopped(round limit)映射Settled Escalated。 |
| `route_selection_checks_any_explicit_route_and_requires_multiple_exit_choice` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L458) | 纯Graph/快照事实测试原业务断言不变。 |
| `snapshot_admission_preserves_module_rounds_and_rejects_unsafe_shapes` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L480) | 纯Graph/快照事实测试原业务断言不变。 |
| `expanded_graph_snapshot_fixture_preserves_node_policy` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L500) | 纯Graph/快照事实测试原业务断言不变。 |
| `file_run_store_read_leases_coexist_and_each_blocks_writers` | [migrated_leases.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_leases.rs#L2) | 只改为FileRunLeaseProvider；原共享/排他flock顺序与互斥断言不变。 |
| `file_run_store_read_and_write_leases_preserve_lock_identity` | [migrated_leases.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_leases.rs#L53) | 只改为租约provider；原inode不删除与OS共享/排他互斥断言不变。 |
| `file_run_store_read_and_write_leases_reject_invalid_run_ids` | [migrated_leases.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_leases.rs#L105) | 只改为租约provider；所有非法id与零目录副作用断言不变。 |
| `file_run_store_roundtrips_rejects_corruption_and_releases_lease_after_process_crash` | [migrated_leases.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_leases.rs#L140) | 租约provider保留原跨进程锁测试；resource roundtrip/损坏JSON改测真实Kernel FileStore，不保留旧store。 |
| `file_run_store_rejects_damaged_result_and_edge_facts` | [migrated_validation.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_validation.rs#L148) | 复用GraphRunRecord纯validate及生产scheduler读取边界；result/key/commit/序列/未知边/反序结果均拒绝且不派发。旧status+cursor组合已不可表达，统一旧字段具名拒绝由schema测试覆盖。 |
| `file_run_store_rejects_edge_decision_not_newer_than_its_result` | [migrated_validation.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_validation.rs#L89) | 原edge/result新鲜度规则由GraphRecord纯validate保持；FileStore只保存opaque resource，Run读取边界负责校验。 |
| `file_run_store_lease_process_helper` | [migrated_leases.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_leases.rs#L270) | 原holder/contender子进程协议不变，改用FileRunLeaseProvider。 |
| `file_run_store_recovers_completed_node_and_artifact_facts_after_process_crash` | [migrated_crash.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_crash.rs#L2) | 真实子进程exit71/72，实际Kernel FileStore reopen；frontier重建key，durable fact/artifact重用，单执行单冻结。 |
| `file_run_store_recovers_not_started_invocation_after_process_crash` | [migrated_crash.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_crash.rs#L94) | 真实子进程exit70；未开始slot从frontier重建同key，只完成一次。 |
| `parallel_wave_recovers_each_branch_after_process_crash` | [migrated_crash.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_crash.rs#L174) | 真实并行子进程exit70/71/72；activation及branch prefixes保存，frontier重建pending，恢复后所有branch/join结果各一次。 |
| `file_run_store_recovers_durable_failed_node_without_replay_or_freeze` | [migrated_crash.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_crash.rs#L250) | 真实子进程exit73；读取durable Failed fact，Code Settle，无重复执行/冻结/edge。 |
| `graph_runner_recovery_process_helper` | [migrated_crash.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_crash.rs#L345) | 保留全部四处process::exit故障点，执行器改为生产KernelScheduler，实际FileStore持久化。 |
| `graph_runner_preserves_recorded_runtime_scenarios` | [migrated_scenarios.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_scenarios.rs#L2) | 仍遍历原10项fixture，派发/结果/ceased/edge等断言保留；S6仅预算未完成passes从1改0、cursor改frontier；phase替代旧标签。 |
| `created_records_carry_started_and_updated_timestamps` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L516) | 纯Graph/快照事实测试原业务断言不变。 |
| `an_old_shape_snapshot_reproduces_its_stored_digest` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L528) | 纯Graph/快照事实测试原业务断言不变。 |
| `newer_run_formats_are_refused_instead_of_guessed` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L577) | 纯Graph/快照事实测试原业务断言不变。 |
| `reentering_a_terminal_run_does_not_advance_the_write_stamp` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L293) | 真实完成后重复scheduler.run，resource与history逐值不变，node不重派发。 |
| `persist_stamps_updated_and_keeps_memory_in_step_with_disk` | [migrated_lifecycle.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_lifecycle.rs#L312) | 旧Runner.persist消失；GraphRecord.touch仅观察字段，实际Kernel FileStore roundtrip及started不改断言保留。 |
| `old_run_formats_are_named_refusals_on_decode_and_file_load` | [migrated_storage.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_storage.rs#L204) | 全部1..13格式及旧environment/status/cursor/authorization/deleted Call具名拒绝；Kernel opaque状态不改写。 |
| `old_cursor_counter_timing_is_named_refusal_without_rewrite` | [migrated_storage.rs](../../../rust/anchor-runtime/src/graph/tests/migrated_storage.rs#L256) | 旧cursor一律具名拒绝；实际scheduler读取FileStore报错，resource不改、零node calls。 |
| `run_record_decode_preserves_duplicate_field_rejection` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L590) | 原重复字段规则不变，仅当前format字面值13更新14。 |
| `non_object_run_record_decode_requires_offline_migration` | [tests.rs](../../../rust/anchor-runtime/src/graph/tests.rs#L627) | 原非object具名拒绝不变；保留历史字段顺序fixture。 |

原始验证输出和最终结果在 `runtime-removal-checks.json` 及其列出的日志。首快照提交仍可用原生成器复现，最终分支不恢复旧执行器。
