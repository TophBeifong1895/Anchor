# M2 收尾规则清单

比较基线为 §10.11 合并提交 `490c000`，逐个规则体核对当前实现；不是复用 M1.5 的旧包络或 master 的 36→13 数字。行号及整文件 SHA256 见 [completion-counts.json](completion-counts.json)。表中规则包含重叠子条件，不相加为总量；同一派生函数的调用、测试兼容 helper、Phase/status 的格式映射均不重复计数。

| 规则 | Host 改前 → 改后 | 当前独立位置（仓库相对路径） |
| --- | --- | --- |
| 是否终态：生命周期谓词 | 1 → 1 | `rust/anchor-runner-host/src/application/run_access/run_view.rs:35` |
| 是否终态：已序列化相位的未结束投影 | 1 → 1 | `rust/anchor-runner-host/src/application/run_access/projection.rs:9` |
| 可否 stop：动作投影、旧路径非活动写门禁 | 2 → 2 | `rust/anchor-runner-host/src/application/run_access/run_view.rs:20`<br>`rust/anchor-runner-host/src/application/run_access.rs:420` |
| 可否 resume：动作投影与两处排空门禁 | 3 → 3 | `rust/anchor-runner-host/src/application/run_access/run_view.rs:20`<br>`rust/anchor-runner-host/src/application/run_access/kernel_runs.rs:214`<br>`rust/anchor-runner-host/src/application/run_access/kernel_runs.rs:438` |
| 可否 abandon：未结算 | 1 → 1 | `rust/anchor-runner-host/src/application/run_access/run_view.rs:20` |
| 可否 pause：动作投影、非活动旧路径、Kernel 请求边界 | 3 → 3 | `rust/anchor-runner-host/src/application/run_access/run_view.rs:20`<br>`rust/anchor-runner-host/src/application/run_access.rs:420`<br>`rust/anchor-runner-host/src/application/run_access/kernel_runs/pause.rs:90` |
| 是否静止 | 1 → 1 | `rust/anchor-runner-host/src/application/run_access.rs:237` |
| 是否需要关注 | 1 → 1 | `rust/anchor-runner-host/src/application/run_access.rs:251` |
| 迟到结果的写入许可 | 0 → 0 | 无：委托 Kernel |
| 未知效果：恢复决定门禁 | 1 → 2 | `rust/anchor-runner-host/src/application/run_access/recovery.rs:120`<br>`rust/anchor-runner-host/src/application/run_access/kernel_runs/recovery.rs:75` |
| 未知效果：普通 resume 的额外阻挡 | 1 → 2 | `rust/anchor-runner-host/src/application/run_access.rs:420`<br>`rust/anchor-runner-host/src/application/run_access/kernel_runs.rs:438` |
| 未知效果：隐藏普通 resume 动作 | 0 → 1 | `rust/anchor-runner-host/src/application/run_access.rs:270` |
| 未知效果：原生工具 result 缺失的事实识别 | 0 → 1 | `rust/anchor-runner-host/src/goose_acp/recovery.rs:6` |
| 未知效果：派发前的原生观察核查 | 0 → 1 | `rust/anchor-runner-host/src/node_host/execution.rs:232` |
| 预算耗尽：旧路径恢复策略门禁 | 1 → 1 | `rust/anchor-runner-host/src/application/run_access.rs:436` |
| 子 Run 结算后释放父 Run：旧路径与 Kernel 各自匹配持久绑定 | 2 → 2 | `rust/anchor-runner-host/src/application/run_access/dispatch.rs:112`<br>`rust/anchor-runner-host/src/application/run_access/kernel_calls.rs:333` |

这是双路径能力补齐，不能解释为规则数量下降。stop/resume/abandon 的最终写入许可由 Kernel 裁决；Host 的动作是观察投影。两处 Kernel resume 排空检查分别是准备前快速检查和注册锁内复核，M3 可进一步收口；未知效果在执行前核查、恢复写入和动作显示分别约束不同操作，不把它们冒充一个重复状态集合。

层次来源（不并入 Host 计数）：Kernel 的 settled/waiting 写门禁在 `rust/anchor-kernel/src/kernel.rs:214`、`:227`，生命周期控制在 `:246`，成本与预算在 `:457`；本轮均未变。Runtime 的端口 BudgetExhausted 映射为 `rust/anchor-runtime/src/graph/runner.rs:563`、`parallel.rs:242` 和新增 `scheduler/execution.rs:153`；子 Run 释放的提交映射为 `scheduler/calls.rs:179`。这些是不同层的裁决或事实映射，不把共享调用算为 Host 独立决定。

Runtime 层仍保留旧 runner/parallel 作为默认与差分对照；新 scheduler 只把端口事实映射为提交。预算结果旧 runner/parallel 各有一处，新 scheduler/execution 增加一处 Code + Observation stop，成本由 submit 参数盖章。迟到结果的决定仅在 Kernel；scheduler 只消费 TaskWaiting/TaskSettled 裁决并排空，保留拒绝与费用。GraphValidator 负责槽位、路由、模块与 Call 结果裁决，本轮没有改变这些定律。

未知效果的旧 record_recovery_decision 保留原行为；新增 Kernel recover_attempt 在提交前要求端口先持久化观察，Goose attempt 识别与 execute 前核查属于可替换执行边缘。完成说明不等于工具成功或模型完成；没有自动重放。普通 resume 的额外阻挡与前端动作投影使用同一份 pending 历史事实，但各有写前门禁/显示条件，分别列明。

Session 的当前助手、已被替代 Turn、作用域、投递与 suppression 仍归 Session；身份、冻结 Graph 和插件授权归 Host 权限边界。调用这些规则不算 Run 生命周期的独立定义。M3 的“默认切换并删除”和 §10.5 Session 单写入者检验尚未开始；本轮不宣称已经兑现最终收敛。
