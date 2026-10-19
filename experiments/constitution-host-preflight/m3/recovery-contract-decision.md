# M3 普通 Run 恢复契约：待裁定

任务书 §8 要求业务行为断言不能放宽；本项触发该停止条件，尚未收到用户裁定。M3 实现不能据此宣称验收通过。恢复、工具效果与相关测试保持当前已提交版本，以下修复方向尚未实施。

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

用户选择之前，不修改这两组恢复业务断言，不变更普通 Run 的恢复控制语义。首轮完整固定 Goose 和逐项独立复跑结果另见 [验证](verification.md) 与 [结构化检查](checks.json)。
