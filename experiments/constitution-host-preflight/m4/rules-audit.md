# M4 规则与归属复核

基点 master 2cef07b。M4 不改 Kernel、GraphValidator judge/prepare/parallel_frontier、Graph logic 或 Session 控制规则；只把已经由 Kernel/Host 拥有的查询投影从 GraphRunRecord 移出。计数沿用 [M3 口径](../m3/rules-session-audit.md)：独立规则体，不是消费者数量。Host 路径相对于 rust/anchor-runner-host/src/。

| 规则 | 改前 → 改后独立判断体 | 当前位置及 M4 差异 |
| --- | --- | --- |
| 是否终态 | 1 → 1 | application/run_access/run_view.rs:35（is_settled）；Kernel Resource::is_settled 是最终裁决，外层委托。 |
| 可否 stop | 1 → 1 | run_view.rs:20（actions）；写入 Human Request 仍由 Kernel 裁决。 |
| 可否 resume | 1 → 1 | 同一 actions；无 worker 的 Executing/Human wait；未知效果显式核查接续不改。 |
| resume 排空前置条件 | 1 → 1 | kernel_runs.rs:184（ensure_idle）；控制和 worker 注册共用 active map。 |
| 可否 abandon | 1 → 1 | actions 复用 is_settled；未结算可放弃，保留 S27。 |
| 可否 pause | 1 → 1 | actions；kernel_runs/pause.rs 复用，边界消费持久 pause intent。 |
| 是否静止 | 1 → 1 | run_access.rs:225（is_quiescent）；has_pending_recovery 读取 RunView，原布尔语义不变。 |
| 是否需要关注 | 1 → 1 | run_access.rs:240（needs_attention）；只读派生，未保存第二份 attention。 |
| 迟到 Work 结果 | 0 → 0 | Host 不重新裁决；Kernel waiting/settled/version 门禁，Runtime submit_with_retry 保留成本与拒绝历史。 |
| 迟到子回调及子 Run 结算释放父 Run | 1 → 1 | kernel_calls.rs:333（kernel_child_finished）；绑定/id/invocation/worker 核对，delivery 后才释放。 |
| 未知效果人工决定目标 | 1 → 1 | kernel_runs/recovery.rs:153（recover_attempt）；完整 key+attempt/Agent/idle/Human wait，先端口持久化再控制。 |
| 未知效果显式核查接续 | 1 → 1 | run_access/recovery.rs:70（inspect_interrupted_attempts）；普通 resume 和 Session-call 共用。 |
| 未知效果观察登记 | 1 → 1 | kernel_runs/recovery.rs:13（observe_recovery_attempts）；复用 Runtime observation_stop。 |
| 未知效果读取投影 | 1 → 1 | kernel_runs/recovery.rs:78 的 recovery_facts 原算法抽为 :90 的 project_recovery；在线和只读磁盘共同调用，不复制判断。 |
| 预算耗尽恢复门禁 | 0 → 0 | 不增加 Host 预算阻挡；Runtime BudgetExhausted → Code Observation stop 的唯一映射未变，W4 不处理。 |
| JSON 未结束识别 | 1 → 1 | run_access/projection.rs:7（projected_run_is_unfinished）；公开 phase 识别不变。 |

C2 审计范围仍为生产 Host/GraphHost 对 GraphRunRecord 的直接保存和字段 mutation，准入后只经 Kernel::submit；[52 项去向](../m3/c2-destinations.md)不变。本轮原有 reads.rs 把 history 塞入 record 的两字段整体赋值被删除，未增加可写字段入口。围栏继续维持 0，未扩豁免。

## 宪法条件与四个极限

- 组件归属：GraphRunRecord/format/sequence 属 Graph 校验器事实；恢复视图和 HTTP 键属可替换 Host 读取边缘；完成事实属 NodeExecutionPort；没有新增恢复引擎或另一套 Agent loop。
- 核心是否需要改动：不需要，anchor-kernel 相对 0859373 零 diff；不新增 Reason/Waiter/Phase/Store/Stamp 接口或规则。
- 核心是否可删：本轮未发现有证据可删的 Kernel 责任，也未用 Host 字段删除冒充 Kernel 最小性的证明。四个指定 CompletionFact 无可删，逐变体证据在字段表。
- 智能无限：投影只消费宿主盖章的历史与结算，提议与裁决分离，不依赖模型自述或具体 Provider。
- 规模无限：仍使用既有逐 Run history 读取，没有扩展全局日志；没有执行大历史规模压测。
- 时间无限：恢复依赖已提交历史与执行端口耐久事实；未观察到的结果保持未知，未伪造完成或旧更新时间。
- 存在对手：身份由宿主盖章、旧 schema/旧字段具名拒绝，History 只取已接受且身份匹配的观察；HTTP/Web 不获得写入或授权权威。平台与独立宿主仍共用同一 Kernel/GraphValidator/KernelScheduler。

四极限为本轮结构复核，不代表新增长期/规模/真实模型 Provider 的实证验收；M5 度量未自动启动。
