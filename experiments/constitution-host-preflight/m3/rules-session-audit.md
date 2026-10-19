# M3：规则与 Session 事实审计

本表核对 M2 已验收提交 `4d3d19d4ba120a85526c7d5a8abc00ef0c777eec` 与 M3 当前实现，落实任务书 §10.1、§10.5、§10.13。它记录源码中实际的判断与所有权，不宣布 M3 验收通过；workspace、Goose、master 合并和真实 Provider 的执行结果必须由最终交回证据分别说明。

## 计数方法

单位是独立写出的规则体，不是调用该规则的函数数或 `rg` 命中数。生命周期动作建议、worker 互斥、工具效果观察、Session 投递是不同约束，分行列出；不把它们加总成一个“重复规则数”。同一视图或 helper 的消费者不增加计数，写前校验若自行重写同一公式则仍计数。测试、纯相位序列化和字段赋值不计规则。

改前数及定位来自 [M2 规则清单](../m2/completion-measurement.md) 和 [逐位置数据](../m2/completion-counts.json)，同时以 `git show 4d3d19d:<path>` 复核旧控制/恢复与新旧双路径。下表拆分沿用该口径，没有把 M1.5 较宽的状态包络数字拼接为 M3 基线。当前行号可能随格式整理移动，函数名是稳定定位。以下 Host 路径相对于 `rust/anchor-runner-host/src/`。

## 八类 Run 规则

| 规则及独立子规则 | Host 改前 → 当前 | 当前唯一规则体或保留的独立位置 |
| --- | ---: | --- |
| 1. 是否终态：生命周期谓词 | 1 → 1 | `application/run_access/run_view.rs:35`，`RunView::is_settled`；外层视图、准入、放弃、删除和 Session 读取它，不各自定义终态集合。Kernel 自己的 `Resource::is_settled` 是核心规则，调用不算 Host 新定义。 |
| 1. 已序列化列表是否未结束 | 1 → 1 | `application/run_access/projection.rs:7`，`projected_run_is_unfinished`；仅识别公开 executing/waiting，相位缺失或旧 status 不获得调度许可。这是 JSON 输入边界的独立判断，未把它藏在消费者数字中。 |
| 2. 可否 stop：动作建议与旧 inactive 写门禁 | 2 → 1 | `application/run_access/run_view.rs:20` 的 `actions`；旧 `validate_inactive_control` 已删除，实际 stop 由 Human Request 交 Kernel 决定。 |
| 2. 可否 resume：动作建议 | 1 → 1 | 同一 `actions`，Executing/Human wait 且无 worker。Child/Effect 等待不被显示为普通 Human release。 |
| 2. resume：worker 排空前置条件 | 2 → 1 | `application/run_access/kernel_runs.rs:177` 的 `KernelRuns::ensure_idle` 是唯一规则体；`control` 锁内复查和 `kernel_control` 准备前检查共用它。同步注册由同文件 `register:195` 的单一 active map 负责，不是第二份 Run phase。 |
| 2. 可否 abandon：未结算建议 | 1 → 1 | `run_view.rs:20` 的 `actions` 复用 `is_settled`；真正写入经 Kernel Human abandon。相同 reason 重试与 AlreadyAborted 响应见下文 Host 命令收窄。 |
| 2. 可否 pause：建议、旧 inactive 门禁与请求复核 | 3 → 1 | `run_view.rs:20` 的 `actions().pause` 是唯一公式；`application/run_access/kernel_runs/pause.rs:90` 的 `request_pause` 在 registry 锁内投影当前事实并调用同一个动作派生。旧 inactive pause helper 和重复公式已删。 |
| 3. 是否静止 | 1 → 1 | `application/run_access.rs:207`，`RunView::is_quiescent`：无 worker、无 pending recovery，且 Settled/Human wait。Session 的 paused/budget Turn 占用另外收窄，不改 Run 相位。 |
| 4. 是否需要关注 | 1 → 1 | 同文件 `needs_attention:222`：等待、非 Done 结算、Executing 且无 worker。当前保留为读取门面契约，未保存第二个 attention 字段。 |
| 5. 迟到 Work 结果的写入许可 | 0 → 0 | Host 不决定；Kernel 的 waiting/settled/version 门禁决定。scheduler 排空任务并保留拒绝历史、实际成本，StaleVersion 重提成本归零。 |
| 5. 迟到子回调的绑定保护 | 1 → 1 | `application/run_access/kernel_calls.rs:269`，`kernel_child_finished`；要求子已结算、父无 worker、父仍等该 child，node/invocation/child id 与持久绑定一致。此规则同时服务第 8 类，不加总。 |
| 6. 未知效果：人工决定目标门禁 | 2 → 1 | `application/run_access/kernel_runs/recovery.rs:75`，`recover_attempt`；匹配完整 InvocationKey + attempt、inactive Human wait、AgentNode，持久化观察后才提控制提交。旧 recovery target/cursor 门禁已删。 |
| 6. 未知效果：普通 resume 的写前阻挡 | 2 → 1 | `application/run_access/kernel_runs.rs:504`，`kernel_control` 读取 `recovery_facts`，仍有 pending 不续跑；旧状态分支已删。 |
| 6. 未知效果：隐藏普通 resume 动作 | 1 → 1 | `application/run_access.rs:241`，`can_resume` 复用 `has_pending_recovery`。这是建议层收窄，不拥有恢复事实。 |
| 6. 原生工具 result 缺失的事实识别 | 1 → 1 | `goose_acp/recovery.rs:6` 的 `attempt`；工具观察存在而 result/模型 completion 未完成时形成待核查事实，属于执行边缘。 |
| 6. 派发前原生观察核查 | 1 → 1 | `node_host/execution.rs:239` 的 `NodeExecutionPort::execute`；发现 pending 时返回 WaitingRecovery，不派发工具。 |
| 7. 预算耗尽：旧路径额外恢复策略门禁 | 1 → 0 | 旧 `validate_resume_budget` 删除。端口预算观察映射为 Human wait(reason=budget)，由已有 Kernel Human resume 裁决，不在 Host 保存 BudgetStopped。Session 的 Turn 占用与 continuation 策略见下表。 |
| 8. 子 Run 结算后释放父 Run：Host 匹配持久绑定 | 2 → 1 | 旧 `dispatch::child_finished` 和 cursor 重入删除；只留 `kernel_calls.rs:269` 的 `kernel_child_finished`。`release_parent_after_recorded_delivery`、驱动退出的 recheck 和 detached callback 都调用这一入口。 |

生命周期建议、worker 排空、pause 请求均共用对应的一份派生函数；消费者和锁内再次调用不增加独立实现数。JSON 输入识别保留在门面协议投影处并单列，严格识别公开相位，不能从旧 status 或缺失投影推断执行许可。Session、Call 执行选择与权限收窄逐项列在下文，不混入生命周期定义，也不隐去真实存在的门禁。

### 最终裁决与边缘映射

以下不是 Host 规则副本，也不计入上表，但决定其含义：

| 层 | 规则、当前位置与 M3 去向 |
| --- | --- |
| Kernel | `rust/anchor-kernel/src/kernel.rs` 的 waiting、settled、version、控制身份和成本预算门禁不变。Host 的动作建议不能授权提交，更不能覆盖已结算事实。 |
| Runtime 迟到 | `rust/anchor-runtime/src/graph/scheduler.rs:48` 的 `submit_with_retry` 只处理新版本重提与一次计费；已等待/结算的结果交 Kernel 留史。没有第二个生命周期 writer。 |
| Runtime 预算 | 端口 BudgetExhausted 映射从旧 runner、旧 parallel、新 scheduler 三处变为 `scheduler/execution.rs:167` 一处 Code + Observation stop；Kernel 账本硬上限 Escalated 是另一层，未改。W4 的默认有限预算不在本轮擅自补齐。 |
| Runtime 未知效果 | `scheduler/execution.rs:36`、`:161`、`:174` 分别把未知 completion、WaitingRecovery、Interrupted 转为观察与控制提交；没有把未知当成功。Cancelled/Suspended 只排空 worker，Kernel Executing 与同一 frontier 保留。 |
| Runtime 子等待 | `scheduler/calls.rs:145` 的 `observe_child` 观察同一个 child，先以 Code Request 释放该 Child wait，再提交 Call 结果；身份和绑定仍由 Kernel/GraphValidator 验证。Host 回调只安排 drive。 |
| Graph Host | `rust/anchor-graph-host/src/graph_call.rs:535` 的 `project_child_outcome` 把 Kernel Done/Waiting/Unknown/其他结算映射为端口观察并导出产物，不写 Run。该文件 `:319` 的可重新 drive 相位集合、Host `kernel_calls.rs:184` 的 Human wait 保持停放、`:245` 的 detached Executing 门禁、`kernel_runs.rs:551` 的 standalone drive 选择是执行适配约束，全部列明，不冒充普通 Human resume 许可。 |
| 执行边缘 | `node_host/facts.rs:16`、`node_host/execution.rs:170`、`goose_acp.rs:235`、`:361`、`:387` 继续识别缺失终结、Host/Goose 冲突和传输中断；这些是不同来源的观察边界，不回写 Run 状态。Goose 原生 Session 与 ACP 仍负责执行与压缩。 |

### Session 和权限的动作收窄

`application/run_access/projection.rs:67` 在生成建议前重读权威视图。冻结 Graph 身份、metadata digest、当前 assistant、已被后继替代的 Turn、abandon intent、Session-call 协调分别限制用户可用操作；执行用例再次验证，不靠 JSON 建议授权。对应规则是 `validate_control_identity`、`check_execution_scope`、`ensure_current_assistant`、`reject_superseded_conversation_execution`、`control_expected` 与 `prepare_resume`。

`application/abandon.rs:111` 只负责 Host 命令的同 reason 幂等和响应：已有 Aborted 可报告 AlreadyAborted，其他结算不能反转；reason/requested_at 保存在 immutable intent，最终结算仍由 `finish_kernel_abandon → KernelRuns::control` 申请。`finish_kernel_abandon` 提交后及已 Aborted 的幂等成功分支都等待同一 registry 的 `wait_idle`，返回时 worker/sandbox 已退出；这是完成命令的同步语义，不是新的状态源。pause 文件同样是待消费命令，派发边界才将其转为 stop(reason=paused)，不承担第二份 Run 状态。

## §10.5：Session 逐位置核查

分类 A 是读取 Run 的权威事实；分类 B 是 Session 自己的事实或独立权限收窄。凡同一流程同时涉及两者，分别说明。C 表示同一事实有两个权威写入者，需要纠正，不能仅靠换字段名消除。

| 位置 | 读取之后的业务决定 | 分类、事实所有者与当前写入路径 |
| --- | --- | --- |
| `application/run_access/conversations.rs:18`，`channel_turn_status` | Run 结算/显式停止映射为 Turn Completed/Failed/Stopped；assistant 的 Done 不结束整个 Turn。 | A→B。Run Phase 只读；Turn 由 `SessionStore::finish_turn` 校验与保存。paused/budget 和有 pending recovery 的 Run 不提前结束 Turn。 |
| 同文件 `settle_channel_run:66` | 检查 current assistant、superseded inbound，再结束对应 Turn；成功回复可申请投递。 | A→B。current pointer、Turn、inbound 与 delivery 由 SessionStore 拥有；不会写 Run。助手正常回复由回复事件结束 Turn，助手 Run 本身继续接收输入。 |
| 同文件 `wecom_channel_delivery_request:32` | 从已接受 reply result 生成 content digest 与幂等 delivery key。 | A→B。冻结回复是 Run 结果；投递申请和结果是 Session delivery 事实，不把 Run Done 当外部投递成功。 |
| 同文件 `ensure_wait_closure_settled:216` | 新 Turn/删除前遍历 wait 子闭包；paused/budget、pending recovery、尚未静止则保留当前 Turn。 | A+B。Run 静止复用门面；Session handoff 的占用策略独立。重读当前视图，旧投影不能放行；显式 abandon 结算后保留历史但不再当 pending 阻挡交接；缺少 wait child 保守保留，删除闭包才允许已删 child。 |
| 同文件 `successor_reads_predecessor:287` | 后继仍可首轮继承前驱 workspace/history 时保留前驱；缺少 admission 也保留。 | A+B。Run settlement/首轮结果为证据；前驱尚未 commit 的 frontier 只有存在 Host/Goose durable started 事实时才纳入 history 继承保护；lineage 与 tombstone 是 Session 事实。不会通过改 Run 状态来解除引用。 |
| 同文件 `retry_conversation:164`、`admit_conversation:352` | 对重试身份、输入、附件、最新前驱及 Graph 作用域做准入检查。 | A+B。唯一 Run admission 经 Kernel；conversation identity/attachment manifest 归 Host metadata；不建立 Session 自有的 Run phase。 |
| `application/conversations.rs:91`、`:194` | 从 metadata 与 tombstone 建立唯一对话链，判断有无后继。 | B，读取存在性与 frozen 身份，不另判 Run 终态。链顺序来自 predecessor，不按时间排序猜最新。 |
| 同文件 `ensure_conversation_delete_allowed:263`、`ensure_current_assistant_is_replaced:317` | 有 worker、仍被后继继承、current assistant 或未解决 delivery 时不删。 | A+B。worker 来自注册观察；继承/当前实例/投递引用归 Session/Host 自有事实，Run 删除经门面预留。 |
| 同文件 `superseded_conversation_ancestor:337`、`reject_superseded_conversation_execution:363` | 已有后继的 Turn 及其 wait 子孙不可恢复；detach 边界可独立继续。 | B，Session 线性化与独立授权范围。没有把“被替代”写成 Run terminal。 |
| 同文件 `check_execution_scope:375` | 同 Graph/Session 不允许并行占用当前执行，独立非对话 Run 与对话 Run 互斥。 | A+B。`is_settled`、`occupies_conversation_execution` 来自门面；作用域与 current head 是 Session/准入规则。 |
| `application/assistants.rs:16`、`:40` | 静止后可 retire；旧实例不能继续作为当前助手。 | A+B。Run 静止只读；SessionStore 是 current assistant 指针唯一权威。retire 不结算 Run。 |
| `application/run_access/graph_policy.rs:14` | 重试 assistant 准入需身份一致；未停止的旧实例保留，不能另开新实例绕过。 | A+B。Run settlement/stop 从视图读取；SessionStore 绑定 current assistant；新 Run 只走 Kernel create。 |
| `application/run_access/session_calls.rs:9`、`:92` | Session continuation 需要尚可继续、无 budget hold，身份/链头/Graph 作用域一致；pending recovery 经认证 Session 协调逐项保存核查提示后继续。 | A+B。协调权归认证 Session host，普通 pause/resume 不得替代。执行阶段复用 recover_attempt 和端口观察后提交 Kernel；不清空恢复数组，不重放旧工具。 |
| 同文件 `settled_session_call_successors:17` | 只有已停止执行的后继可作为 Session-call 的对话继承链。 | A+B。Run 的 stopped/settled 派生只读；Session 链仍归 metadata。 |
| 同文件 `stop_session_call:65`、`yield_session_call:80` | stop/yield 通过 Kernel 控制；stop 另结束 pending delivery。 | A+B。Run phase 归 Kernel；metadata 的 delivery failed 表示 Session 协调结果，和 Run settled 是不同事实。yield 不制造 delivery 完成。 |
| 同文件 `settle_session_call:193` | worker 已退出、Done 才能 delivered；相同决定幂等，不同决定冲突。 | A+B。Session delivery outcome 由此协调用例写 metadata；停止路径在相同 admission 串行域内写同一 delivery，非独立事实副本。然后调用共用 child callback。 |
| `application/session_calls.rs:71`、`:87`、`:108`、`:132` | pending delivery 保留 Run；父 stop 处理 wait delivery；登记 Session call；投递结果映射父调用。 | B。SessionCall context/delivery 状态独立于 Run phase；detach 不受 wait 父取消。认证 HTTP 适配只调用这些用例。 |
| `application/run_access/kernel_calls.rs:269` | Session child Run 已结算但 delivery 仍 pending 时，不唤醒父完成；待 delivery 记下后重查。 | A+B。Run settle 与 delivery settle 分别读取；唯一父 wake 门禁核对持久绑定，真正 release 是 scheduler 提交。 |
| `application/run_access/channels.rs:27`、`:78`、`:101`、`:244`、`:260` | 回复是否可用、启动 Turn 收尾、是否保留助手、进度流是否闭合。 | A→B。各用途返回 Session/展示投影；`channel_progress_is_settled` 对停止/预算等待闭合进度流，对 paused/pending recovery 保留，并不把 Human wait 改为 Kernel Settled。错误/缺失的进度闭合策略也不是执行许可。 |
| `api/turns.rs:21`，`recover_pilot_turns` | 启动时恢复 delivery、补关联 Run、按门面建议收尾 Turn，保留可恢复 current assistant。 | A→B。所有 Turn/delivery/association 写入都经过 SessionStore；此处没有 Run 字段 setter。 |
| `api/assistant_recovery.rs:105` | 无 worker 的 current assistant 在保留身份与前驱下 handover 为新实例；metadata 存在而 Run 缺失时保留现场。 | A+B。worker/已准入存在性只读；handover 与 claimed Turn 移交由 SessionStore 原子处理，不把旧 Run 观察当 Goose 恢复游标。 |
| `api/wecom.rs:592`，`wait_for_channel_reply` | Run reply 必须匹配 Turn/inbound，suppressed 不投递；新鲜 reply 可完成 Turn；失败/失去 worker 返回停止。 | A→B。读取 `ChannelReplyRun`，用 SessionStore 的同一 `finish_turn` 保存 Turn；正常 assistant reply 与整个 Run 结算是不同触发。superseded/Interrupted Turn 优先，迟到回复不能覆盖它。 |
| `api/wecom_progress.rs:251` | Run 工作进度闭合时结束 Session 的 SSE 展示。 | A→B。只消费 `channel_progress_is_settled`，没有新增状态公式或持久写入。 |
| `node_host/conversation.rs:124`、`:270` | 只为允许的前驱挂载历史；返回门面的最新投影与已接受 invocation/trace。 | A+B。历史可见性与继承范围由 Session 绑定；Run 事实只读。`settled_session_call_successors` 也复用门面。 |
| `assistant.rs:316`、`:377`、`:419` | 验证 handover Graph/节点/Session 身份，挑已初始化的前驱 workspace。 | B，读取 Run frozen progress/首轮证据，未重新定义 lifecycle。workspace/native history 不是第二份 Run 状态。 |
| `conversation_resources.rs:110`、`:146`；`api/sessions.rs:105` | 引用存在或 metadata 已冻结但 admission 缺失时保留 Session/Graph 资源，阻止误删。 | B，资源引用与 Session 删除边界；不从文件缺失推断完成。 |
| `api.rs:725`，`session_plugin_declarations` | 从冻结 Graph 声明与 Session switches 投影 Plugin 挂载/required。 | B，授权规则，不是 Run lifecycle 规则；读取 progress 不能授权新 Plugin，也不拥有 Run settlement。 |
| `application/run_access/assistant_policy.rs:21`、`:54` | 从当前 frontier/已接受结果提取 Turn binding，用于操作收窄和 Plugin 策略。 | B，Session binding 事实和授权策略；frontier 是临时派生，不保存 cursor。 |

本次未发现仍由两个存储各自持有并更新同一 Run phase 的 C 类位置。最接近旧重复 writer 的三处已经移除：Session-call 自动 `mem::take(recovery)`、伪造 Completed recovery submission、直接写 Running/Stopped。现在 Run 的控制与恢复只提交 Kernel；工具事实由 Goose/执行端口保管，人工决定以引用同一 attempt 的提交登记，Session 不重新编造工具结果。

多个调用者使用同一 `SessionStore::finish_turn` 并不自动代表多个权威 writer：Run 失败收尾、reply 成功、启动重查都在 SessionStore 的状态转换与事件写入处串行校验。SessionCall 的 pending/delivered/failed 保存的是协调投递事实，Kernel outcome 保存执行结算事实；二者故意不同，禁止用一个字段代替另一个。以上是本次源码边界结论，不能据此声称已穷举全部跨进程交错。

## 宪法的三个条件

| 条件 | 当前答案与可核对边界 |
| --- | --- |
| 组件可从宪法推出或可整块替换 | Kernel submit/accepted history 是提议与裁决分离及只追加；纯 GraphValidator 管 Graph 规则；KernelScheduler 只驱动端口与提交观察。RunView/HTTP/Probe 是可重建投影；worker registry、pause/abandon 命令属于 Host 执行协调；Goose ACP、FileStore、HTTP 与渠道为可替换边缘。Session 的 current pointer、Turn 与 delivery 保留各自权威；没有另建 Agent loop、通用日志或恢复引擎。旧 Runner 和双存储归属桥已删除，剩余结构字段收尾明确留给 M4。 |
| 核心是否需要改动 | 本轮无需改动 `anchor-kernel`。审计实际执行 `git diff 0859373 --stat -- rust/anchor-kernel`，输出为空；M3 的公开 Run 路径使用既有 Kernel 能力。零 diff 不替代全量回归，也不意味着未来每个需求都已证明能零改动。 |
| 核心里有没有可删的东西 | 本轮没有发现有证据可删除的 Kernel 职责。身份、结算不可逆、版本、等待者释放、实际成本与效果事实门禁都有现存调用和测试。删除旧 Runner 减少的是 Host/Runtime 的第二套机制，不能当作已经证明 Kernel 最小；本次没有进行 Kernel 删除实验。 |

## 四个极限

| 极限 | 当前结构支持 | 未证明或未关闭的边界 |
| --- | --- | --- |
| 智能无限 | Agent 只能提议结果，不拥有 Human/Code 身份、成本盖章、phase 或批准投递的权力；更强模型仍走同一 Kernel 裁决和 Host 授权。 | 这是所有权检查，没有执行更强模型或真实 Provider 的业务验收；模型质量与有目的的人工选择不能由本表证明。 |
| 规模无限 | 每个 Run 的事实由 FileStore/Kernel 持有；Graph 裁决是纯函数，兄弟并发结果独立提交；一个 Host registry 同步预留 worker，跨进程 writer lease 保持单写入者。 | 当前 Host 单 writer、全量 Run/metadata 扫描、恢复 history 扫描仍有线性成本；30,000 seed 冻结对照与有限线程回归不是无限负载、分布式吞吐或多年历史性能证明。未为此新增平台机制。 |
| 时间无限 | Kernel history 只追加，settled 不反转；迟到结果计费留史；确定性 child id 与完整 InvocationKey 防止旧回调释放新等待。人工观察先持久化再释放，同一决定可幂等重提，未知效果不自动重放。 | 旧开发 Run 具名不接受，按 §10.13 不维护旧读取/生产迁移。Goose 原生会话长期版本兼容、外部工具的长期幂等与 W4 默认有限结算均未由本轮解决。 |
| 存在对手 | 提交身份、权限、版本、路由、绑定 child 与产物 provenance 在既有边界重新校验；会话投递须匹配当前 Turn/inbound，Plugin 显式授权不能绕过准入基线。未知结果不冒充成功。 | 有限单元、差分与本地 fixture 回归不构成完整对抗证明；外部渠道实网与真实 Provider 没有被本审计新增验收。 |

## 证据与交回界限

- [C2 去向](c2-destinations.md) 逐项覆盖 16 save / 36 mutation，当前源码均为 0；不能用规则计数代替写入清零。
- [Runtime 测试去向](runtime-test-mapping.md) 保存 73 个原测试的迁移映射；[删除检查](runtime-removal-checks.json) 保存移除项的静态检查。原始输出位于 `/tmp/anchor-m3-runtime-full-final.log`、`/tmp/anchor-m3-runtime-migrated-final.log`，分别为 Runtime 155 unit + 7 integration + 2 docs 与 graph::tests 98 项，均 0 失败、0 ignored。
- Session 读取与所有权的定向业务断言保存在 `run_access/conversations.rs`、`session_calls.rs`、`channels.rs`、`assistant_policy.rs`，API 原用例仍在对应 `api/tests` 模块；本审计未重复运行 Host 构建，最终结果由主流程给出，不能引用尚未完成的运行作通过证据。
- 文档审计未执行真实模型 Provider、外部投递或部署；本地确定性 Provider、固定 Goose、真实模型验收是不同范围。

结束同步 master 后 API/Host 普通 workspace 已通过，固定 Goose 完整首轮 110 / 12 / 0。未知效果恢复存在业务契约冲突，人工 recovery 另有重入锁；配置、身份预检的已知失败适配与三项旧目录夹具也尚未修复。按任务书 §8 停止，M3 未通过，见 [交回](verification.md) 与 [未关闭回归](goose-regressions.md)。上表是当前代码的独立规则体与所有权计数，不将静态收敛等同为恢复行为验收。
