# M3 固定 Goose：未关闭回归

> 历史现场：本文保留 §8 停止时的 110 / 12 / 0 与原独跑失败。§10.15 已裁定并完成修复，当前 M3 结果为完整固定 Goose 122 / 0 / 0、原 12 项各自独跑通过，见 [§10.15 交回](completion-verification.md)。下文不是当前未关闭待办。

当前源提交 `b61c325`，完整 Host ignored 首轮 **110 passed / 12 failed / 0 ignored**，41 targets、7 个失败 target，退出码 101。完整命令逐套串行，每套单线程；原样结果与日志摘要见 [checks.json](checks.json)。全部 122 项实际执行，不把普通测试的 ignored 数当作已通过。

按任务书 §10.14，12 项随后使用首轮的原测试二进制逐项 `--ignored --exact --test-threads=1` 独立复跑。12 项逐项独立复跑全部再次失败，各为 0 passed / 1 failed / 0 ignored，均退出 101；与首轮相同失败点，确认不是仅由并发负载触发。完整逐项结果见 checks.json。未重建或替换 Host/Goose/helper，身份见 [binary-identities.json](binary-identities.json)。原始日志和失败 fixture 留在本机 `/tmp/anchor-m3/` 及各测试输出的位置，不入库。

## 现场与后续修复边界

下表为只读诊断及修复建议，**本轮尚未实施这些修复**。业务断言、恢复行为和 Kernel/Validator 保持原样；不得把建议写成已经验证的解决方案。

| 失败范围 | 数量 | 现场与建议边界 |
| --- | ---: | --- |
| `goose_acp::native_goose_unknown_external_effect_reopens_same_session_and_checks_before_continuing`；`goose_channel::native_goose_unknown_channel_result_resumes_same_session_and_inspects_without_resending` | 2 | 首次显式 resume 发现 pending 后停驻，旧业务用例要求同一原生 Session 中启动 Agent 核查再继续。与 M2 人工门禁用例的首次 resume 契约冲突；按 §8 待用户裁定，见 [恢复契约](recovery-contract-decision.md)。不得直接改成已完成或重放效果。 |
| `goose_acp::real_goose_external_effect_then_host_kill_is_spike_failclosed_not_native_safe_recovery` | 1 | fixture/version1 的无完成事实被映射为通用 Uncertain，调度器停驻而非原要求的 Failed。建议仅在 spike completion_fact 边缘映射为既有 Failed，不全局改变 Uncertain，不把 spike 写成可安全恢复。 |
| `goose_conversation::missing_predecessor_{fact,scope,model_binding}_fails_closed` | 3 | 身份校验的 CorruptRun 在 ConversationScope::open 调用处直接返回，Run 留在 version0 / Executing；旧外层执行适配会写 Failed。建议只在该身份预检边界转既有 Failed，保留其他 I/O/lease 错误及原生历史、零模型请求断言。 |
| `goose_kernel_recovery::kernel_unknown_native_effect_requires_a_durable_human_decision_before_continuing_or_abandoning` | 1 | /recovery HTTP 超时。recover_attempt 持有 active 锁时回调 prepare_resume，后者 validate_configuration → read_run → view → active 重入同一锁；仅 Code stop 已持久化，native/Kernel 均未记录决定。建议锁外准备并保留 Result，锁内仍先执行原校验、端口持久化及 Human 提交；保持错误优先序、完整 key 校验、幂等、冲突、Abort 与不重放断言。此锁问题独立于普通 resume 的产品契约。 |
| `goose_plugin_configuration::managed_configuration_rotates_atomically_without_restarting_or_changing_live_invocations` | 1 | 前两 Run Done，清空必填配置后第三 Run 停在 Executing，无 Goose fact；已知配置预检错误透传，未作失败提交。建议从原始配置错误点保留明确类型，再转既有 Failed；不能靠 reason 文本或全局 Unsupported 捕获。 |
| `goose_plugin_mcp::graph_mcp_startup_errors_settle_without_repolling_completed_resolution` | 1 | 宿主策略拦截 MCP command，Run 留在 Executing。首断言已经读取 projection.outcome，不是词汇问题。建议明确标记已知启动拒绝/已完成握手失败，保留取消、超时、清理失败和未知路径；首轮尚未到达该测试的后续握手失败分支，不能声称其已验。 |
| `goose_plugin_turns::{add_call_remove_preserves_run_native_history_workspace_and_frozen_run_snapshot,default_restart_handover_restores_same_turn_grants_then_next_turn_revokes,save_during_actual_tool_keeps_current_turn_and_other_users_adopt_independently}` | 3 | 三者在 tests/goose_plugin_turns.rs::records 读取已删除的 state/runs 目录而失败。旧夹具目录读取遗漏，不据此判定 Plugin Turn 行为失效；后续应改用现有 Kernel/Host 只读记录入口，保留原授权、冻结和其他用户隔离断言。第三项退出另伴随事件请求超时，须在夹具修复后继续验证，不能提前算通过。 |

## 交回边界

普通 workspace、Web、clippy/fmt 已通过；以上固定 Goose 失败仍未关闭。M3 因任务书 §8 的恢复业务断言冲突停止，**未达到 §10.13 的 Goose 零失败门槛，不能合回 master**。尚未执行真实模型 Provider、外部渠道实网或部署，不自动推进 M4/M5。
