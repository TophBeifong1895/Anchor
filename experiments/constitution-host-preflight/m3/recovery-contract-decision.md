# M3 普通 Run 恢复契约：§10.15 已裁定

任务书 §10.15 已选择选项 1：普通显式 resume 由 Agent 在同一原生 Session 核查现场后继续；人工 Completed/Abort API 保留为另一条路径。该项登记为“M2 行为更正：恢复回到 master 契约”，不增加 S 候选。以下冲突是上次停止现场，修订后的验证结果由 M3 交回单独说明。

## 同一入口的冲突

| 用例 | 相同现场 | 要求的响应 |
| --- | --- | --- |
| `goose_acp::native_goose_unknown_external_effect_reopens_same_session_and_checks_before_continuing` | 默认原生 Goose，外部工具效果已发生，Host 在收到 ToolResponse 前被杀死，重启后显式请求 resume。 | 一次 resume 后加载同一原生 Session，由 Agent 先核查实际效果，再完成；效果仅一次，新完成必须引用新观察。 |
| `goose_kernel_recovery::kernel_unknown_native_effect_requires_a_durable_human_decision_before_continuing_or_abandoning` | 默认原生 Goose，同样是外部效果已发生、原生工具 result 缺失，重启后显式请求 resume。 | 首次 resume 只发现并持久停驻，不请求模型；第二次普通 resume 被阻挡，直至显式 recovery Completed 或 Abort。 |

两者都要求同一 invocation/Session、不自动重放效果、恢复事实持久化和完成回执有效，但对首次显式 resume 是否可以启动 Agent 核查提出不同要求。`goose_channel` 的未知渠道回执用例也要求一次 resume 后核查而不重发。

## 代码现场

M2 基点 `4d3d19d` 的普通路径由 Goose 载入原生 Session 并要求先核查现场；已登记的 unknown attempt 也可由旧 `resume_interrupted_attempts` 保存核查提示后恢复。M3 的 `node_host/execution.rs` 在所有 Run 派发前查询 `pending_recovery`，发现未知效果就返回 WaitingRecovery；新调度器据此以 Code + Observation stop 保存等待。因此旧普通恢复用例不再向模型发出核查请求。

AGENTS.md 的长期产品原则是保存事实后由 Agent 核查现场继续，不将内部 attempt 包装成用户审批流程。M2 同时明确要求保留人工 Completed/Abort 恢复决定 API；这个 API 的存在本身不能决定首次普通 resume 应停驻还是启动核查。

## 已呈交的选项

1. 保留旧产品行为：显式 resume 可让 Agent 核查后继续；M2 人工决定场景通过另一种准备方式进入已登记的 pending 状态，同时保留人工 API 的不重放、幂等、冲突和 Abort 断言。
2. 保留 M2 门禁：普通 resume 先发现并停驻，必须显式恢复决定；旧普通恢复与渠道恢复需要登记并获批用户可见语义变化。

上次停止时未修改相关业务断言。用户现已明确允许更正首次 resume 的测试准备与动作断言：人工 API 场景在 Host 关闭时以实际 Code + Observation 提交登记 pending，不再把首次 resume 用作停驻触发器；Completed/Abort、不重放、幂等、冲突、效果一次及同 Session 断言全部保留。首轮失败现场见 [停止验证](verification.md) 与 [结构化检查](checks.json)，续做的最终验收单独留证。
