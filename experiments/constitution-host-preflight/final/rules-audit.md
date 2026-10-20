# M5 固定提交规则清单

旧基点为 `1d1ba54035b2f25a45fd52c2536f5dc19cbd85e2`；迁移后基点为 `e60374fd06eda07de8df9c41e64a5e03f6e3246d`。以下行号全部来自这两版 `git show`，不读取测量期间变化的工作区。这里只完成源码复核，不表示真实模型、渠道实网、长期运行或规模压测通过。

## 口径与复核

- 单位是独立规则体。调用同一个 `RunView`、`actions`、派生函数或 Kernel 谓词的消费者不重计；一个 helper 内不同约束分别列出。测试、fixture、纯序列化及字段搬运不计。
- Host 指 `anchor-runner-host` 与 `anchor-graph-host`。表内按生命周期、Host 协调、执行端口分开；Runtime、Kernel、Web 另外列。不同子规则不能求和成“同一规则有几套实现”。同一位置可参与多个规则，不把它们累计为全局重复数。
- 路径缩写：`R/` = `rust/anchor-runner-host/src/`，`G/` = `rust/anchor-graph-host/src/`，`T/` = `rust/anchor-runtime/src/`，`K/` = `rust/anchor-kernel/src/`，`W/` = `apps/web/src/`。每一处 `路径:行` 都应按所在列的提交读取。函数名用作辅助定位。
- 先逐一核实 [M1.5 counts.json](../m15/counts.json) 的全部旧位点，所有原行均与旧 Git blob 一致；再用固定提交检索 `Phase`、旧 status、control、recovery、completion、budget 和 callback，审阅规则体及其消费者，不能把检索命中数当计数。
- 旧表漏列的 JSON 未结束识别 `R/api/schedules.rs:316` 本次补列；旧 detached 扫描 `R/application/dispatch.rs:78` 与实际派发的 `:51` 是两份独立公式，也补列。另将已存在的旧 Agent 状态识别 `R/node_host/execution.rs:191` 显式列入执行边缘，避免用较宽旧表与较窄新表直接宣称未知效果 13→4。
- 可复核脚本为 [rules_audit.py](rules_audit.py)，执行 `python3 experiments/constitution-host-preflight/final/rules_audit.py`。它只读取固定提交，校验全部 M1.5 before 位点、补列位点与最终规则锚点，并向标准输出给出源码 SHA256 和计数；不修改源码、不构建、不跑测试。数字是语义审阅输入，脚本只证明定位与源码身份，不能自动证明穷举。

## 八类规则

### 1. 是否终态

| 独立判断 | 旧基点 | 迁移后基点 | 数量与结论 |
| --- | --- | --- | --- |
| 通用生命周期谓词 | `R/application/admission.rs:64`，准入手写全部未结算 status | `R/application/run_access/run_view.rs:35`，`is_settled`；门面 `run_access.rs:222` 委托它 | 1→1。不是 Host 获得结算权；事实仍来自 Kernel。 |
| JSON 列表未结束识别 | `R/api/schedules.rs:316`，手写旧 status 集合 | `R/application/run_access/projection.rs:7`，只认公开 executing/waiting | 1→1，协议输入边界。M1.5 总表遗漏旧这一处，不能隐去。 |

`K/model.rs:305` 的 `Resource::is_settled` 与 `K/kernel.rs:227` 的 settled 不可逆是核心事实/裁决；不是 Host 的第二份持久状态。旧 `T/graph/state.rs:59` 的 RunStatus 谓词已经删除。

### 2. 可否 stop / resume / abandon / pause

| 独立子规则 | 旧基点位点 | 迁移后基点位点 | 旧→后 |
| --- | --- | --- | ---: |
| stop 建议及生命周期写前门禁 | `R/application/control.rs:87,:224`；`run_projection.rs:108`；`kernel_runs.rs:226` | `R/application/run_access/run_view.rs:23` | 4→1 |
| resume 动作建议 | `R/application/run_projection.rs:90`；`kernel_runs.rs:227` | `R/application/run_access/run_view.rs:24` | 2→1 |
| resume worker 排空门禁 | `R/application/control.rs:87`；`kernel_runs.rs:146,:330` | `R/application/run_access/kernel_runs.rs:184`，`ensure_idle`，控制与准备入口复用 | 3→1 |
| resume 通用终态写门禁 | `R/application/control.rs:224` | 无 Host 重写；最终交 Kernel | 1→0 |
| resume 未知效果直接写前阻挡 | `R/application/control.rs:236` | 无；显式接续先作下述核查 | 1→0 |
| resume 额外预算策略门禁 | `R/application/control.rs:249` | 无 Host BudgetStopped | 1→0 |
| resume 显式核查接续 | `R/application/control.rs:180` | `R/application/run_access/recovery.rs:70`；普通 resume 与 Session-call 共用 | 1→1 |
| 子完成回调重入父 Run 的活跃入口 | `R/application/control.rs:74` | `R/application/run_access/kernel_calls.rs:333`，完整绑定核对 | 1→1 |
| 保留的旧绑定保护源码 | 旧侧与上一行同一体，未额外独立存在 | `R/application/run_access/control.rs:57`，委托 `run_access.rs:281` | 额外 0→1；静态总绑定判断 1→2，当前没有 Some 生产调用也仍计入。 |
| Graph 子 Run 可重新 drive 的集合 | `G/graph_call.rs:342` | `G/graph_call.rs:319`，Executing/Child wait | 1→1 |
| 控制提交或直接 drive 的选择 | `R/application/kernel_runs.rs:350` | `R/application/run_access/kernel_runs.rs:544`；Child wait 先观察子闭包，不能伪造 release | 1→1 |
| abandon 生命周期建议 | 无共享建议 | `R/application/run_access/run_view.rs:32`，委托 is_settled | 0→1 |
| abandon 命令幂等及不可反转 | `R/application/abandon.rs:213,:299`，前置及取消后独立检查 | `R/application/abandon.rs:130`，Aborted 幂等、其余 settled 拒绝；提交后等待排空 | 2→1 |
| abandon 最终直接写状态保护 | `R/application/abandon.rs:159` | 无；`R/application/run_access/abandon.rs:40` 只提 Kernel Human abandon | 1→0 |
| pause 建议及准入 | `R/application/control.rs:87,:242`；`run_projection.rs:106`；`kernel_runs.rs:225,:141` | `R/application/run_access/run_view.rs:22`；`kernel_runs/pause.rs:90` 调同一 actions | 5→1 |

旧 resume 原 12 点在本表按九个不同约束拆分，没有换成消费者包络。迁移后生命周期建议一份、worker 门禁一份；核查、子绑定与执行适配各自仍有明确职责，不能把这些说成“resume 只剩一个判断”。新接入执行端口还有 `R/application/run_access/kernel_calls.rs:241`（Human wait 停放）与 `kernel_runs.rs:615`（独立 Host 只 drive Executing/Child wait），它们是 0→1 的调用适配选择，另外登记，不冒充普通人的 resume 授权。

`R/application/run_access/control.rs:57` 仍保留 `waiting: Some` 时的旧绑定防护，委托 `run_access.rs:281`；所有现有生产调用 `control.rs:18,:22,:26` 都传 `None`。独立判断点按静态源码计入这额外一处，因此绑定规则静态 1→2；活跃回调本身仍为 1→1。M5 只登记，没有删除源码。

pause 的 `kernel_runs/pause.rs:43` 在 dispatch boundary 消费 durable intent 并提 stop(reason=paused)，不是另一处可否 pause 的准入。abandon 的 reason 幂等、排空完成语义和恢复控制仍由 Host 命令拥有；它们不改变 Kernel 结算。Kernel 实际 stop/resume/abandon 裁决分别在 `K/kernel.rs:504,:521,:538`。

### 3. 是否静止；4. 是否需要关注

| 规则 | 旧基点位点 | 迁移后基点位点 | Host 旧→后 |
| --- | --- | --- | ---: |
| 静止 | `R/application/liveness.rs:33`；`kernel_runs.rs:220` | `R/application/run_access.rs:225`：无 worker、无 pending recovery、Settled/Human wait | 2→1 |
| 关注 | `R/application/run_projection.rs:96`；`kernel_runs.rs:221` | `R/application/run_access.rs:240`：Waiting、非 Done settled、Executing 且失去 worker | 2→1 |

产品并非全栈只余一处：Web 仍有 `W/runPresentation.ts:10` 的关注显示和 `:12` 的静止显示。后者没有 pending-recovery 过滤，不能当作 Host 删除/交接许可；Host `needs_attention` 当前保留为门面谓词而没有额外序列化 attention 字段。这里计数描述归属，不把跨语言显示公式称为 Kernel 写入规则。

### 5. 迟到结果

| 子规则 | 旧基点 | 迁移后基点 | 数量与归属 |
| --- | --- | --- | --- |
| 普通 Work 写入许可 | Host 无独立门禁 | Host 无独立门禁 | Host 0→0；Kernel 处理，不能把“没有 Host 公式”说成没有规则。 |
| 活跃子回调的父绑定保护 | `R/application/control.rs:74` | `R/application/run_access/kernel_calls.rs:333` | 1→1；与第 8 类同一规则体，不再次相加。 |
| 保留的旧绑定保护源码 | 旧侧与上一行同一体 | `R/application/run_access/control.rs:57` | 额外 0→1；两行静态合为 1→2，与第 2 类去重。 |

最终 `K/kernel.rs:213,:227,:234` 分别保证 waiting work、settled、stale version 的裁决；`T/graph/scheduler.rs:49` 只在 StaleVersion 后以零额外 cost 重提，`:198` 消费 Kernel TaskWaiting/TaskSettled 拒绝并排空结果。晚到结果的实际成本与拒绝仍留史。Session 迟到回复由 Turn/inbound 绑定决定，见后文，不将投递规则混算为 Run 生命周期。

### 6. 未知效果

以下保留 M1.5 的 13 个旧点及去向，按不同事实来源拆分；新增原生事实边界另列。未知效果不是一个布尔谓词，不能将这些行相加来证明“核心重复”。

| 独立子规则 | 旧基点 | 迁移后基点 | 旧→后 |
| --- | --- | --- | ---: |
| durable started 无终结结果 | `R/node_host/facts.rs:45` | 同路径 `:45` | 1→1 |
| Host/Goose 事实冲突 | `R/node_host/execution.rs:207` | 同路径 `:207` | 1→1 |
| fixture 无 completion 的终结解释 | `R/goose_acp.rs:378` | `R/goose_acp.rs:399`，fixture Failed；native v2 仍 Resumable | 1→1；属于 M1–M4 已交回的边缘调整，不假称行为完全未变。 |
| Goose 未知事实不自动重放 | `R/goose_acp.rs:398` | `R/goose_acp.rs:411` | 1→1 |
| 传输未知与已知合同错误区分 | `R/goose_acp.rs:249` | `R/goose_acp.rs:270` | 1→1 |
| 子 Run 不能自动越过 Human wait | `G/graph_call.rs:342` | `G/graph_call.rs:319` | 1→1；另有 ChildExecutionPort Human 停放 `R/application/run_access/kernel_calls.rs:241`，新 0→1。 |
| 子未知/未返回结果的端口观察 | `G/graph_call.rs:643` | `G/graph_call.rs:535`，Unknown/Executing→Uncertain | 1→1 |
| detached 自动派发前置及启动扫描 | `R/application/dispatch.rs:51,:78` | `R/application/run_access/kernel_calls.rs:310`；`run_access.rs:268` | 2→2；旧扫描一处为 M1.5 漏列。Executing 只表示可 drive，重放仍受端口 durable facts 限制。 |
| 旧 Runner 异常时直接写失败、保留 cursor | `R/execution.rs:307` | 无；`kernel_runs.rs:472` 仅报告 suspended，不能推断并写 Run 失败 | 1→0 |
| 显式 resume 先持久核查提示 | `R/application/control.rs:180` | `R/application/run_access/recovery.rs:70` | 1→1 |
| 当前恢复目标/attempt/Agent/idle 门禁 | `R/application/recovery.rs:219,:107` | `R/application/run_access/kernel_runs/recovery.rs:153`，`recover_attempt` | 2→1 |
| 人工说明后 abandon/继续等待/resume 的选择 | `R/application/recovery.rs:134` | `R/application/run_access/kernel_runs/recovery.rs:223`，仍先持久执行端口事实，再提控制 | 1→1，不能把它藏在 target 门禁里。 |
| 旧 Agent 状态不可冒充 Goose 会话 | `R/node_host/execution.rs:191` | 同路径 `:191` | 1→1，M1.5 未单列的执行边缘。 |
| 原生 tool observation 无 result/completion 的 attempt 识别 | 无 | `R/goose_acp/recovery.rs:6` | 0→1 |
| 原生端口人工决定的目标/Retry/非空说明/冲突保护 | 无 | `R/goose_acp/recovery.rs:34` | 0→1，保护 native fact，不能仅信 Kernel history。 |
| 派发前 pending 原生观察阻挡 | 无 | `R/node_host/execution.rs:239` | 0→1 |
| 显式接续新发现观察的登记与去重 | 无 | `R/application/run_access/kernel_runs/recovery.rs:13` | 0→1，复用 Runtime observation_stop。 |
| 从 accepted history 投影 pending/decisions | 无 Kernel-history 投影；旧 record.recovery 是持久字段 | `R/application/run_access/kernel_runs/recovery.rs:90` | 0→1；在线与只读磁盘共用纯投影。 |

Runtime 的 `T/graph/scheduler/execution.rs:36,:156,:168` 将 Uncertain、WaitingRecovery、Interrupted 转为同一 `observation_stop`；它不假造工具成功。`R/application/run_access.rs:311` 的 has_pending_recovery 是对上述恢复投影与同一 is_settled 的消费，不重计。旧读取状态 WaitingRecovery 和新 Human wait 都不能替代外部效果事实；原生会话与端口仍负责观察持久化。

### 7. 预算耗尽

| 子规则 | 旧基点 | 迁移后基点 | Host 旧→后 |
| --- | --- | --- | ---: |
| 普通 resume 的额外预算策略阻挡 | `R/application/control.rs:249` | 无 | 1→0 |
| 子调用显式允许 BudgetStopped 自动重入 | `G/graph_call.rs:342` | 无 BudgetStopped 白名单；`:319` 对全部 Human wait 停放 | 1→0 |
| 子预算观察转为 Waiting | `G/graph_call.rs:629` | `G/graph_call.rs:600`，统一 Waiting 投影 | 1→1；是共享相位投影，无独立 BudgetStopped 公式。 |

最终 Runtime 唯一端口预算映射在 `T/graph/scheduler/execution.rs:162`，BudgetExhausted → Code+Observation stop(reason=budget)，实际 model_requests 作为 cost 提交。Kernel 自身 `K/kernel.rs:455` 的账本过限结算是另一层定律，不与端口预算混算。Session `can_continue_session_call` 的 budget hold 仍在 `R/application/run_access/session_calls.rs:8`；这是 Session 协调策略，单列保留。W4 是否应结算失败仍待用户决定，M5 没有改裁决。

### 8. 子 Run 结算后释放父 Run

| 独立子规则 | 旧基点 | 迁移后基点 | Host 旧→后 |
| --- | --- | --- | ---: |
| 子结果转换为 Call 观察/产物 | `G/graph_call.rs:577` | `G/graph_call.rs:535`，Done/Waiting/Unknown/其他 settled/Executing 投影 | 1→1 |
| 活跃唤醒前核对 delivery、wait 模式、父 worker、invocation 与持久 child 绑定 | `R/application/dispatch.rs:149`；`application/control.rs:74` | `R/application/run_access/kernel_calls.rs:333` | 2→1；该处与第 5 类相同。 |
| 保留的旧绑定保护源码 | 旧侧包含于上一行 | `R/application/run_access/control.rs:57` | 额外 0→1；合计静态 2→2，当前无 Some 调用不豁免计数。 |

真正 release 在 `T/graph/scheduler/calls.rs:145` 的 `observe_child`：匹配当前 Child wait，提交 Code resume 后再交 Call 结果给 GraphValidator；Kernel `K/kernel.rs:521` 核验等待者身份。Host 回调只安排 drive，不能直接写父 Run 状态。`kernel_recheck_child`、已记录 delivery 的 release、驱动退出回调均复用它。保留但无 `Some` 生产调用的 `control_expected` 分支见第 2 类；没有将残留源码隐去。

## 迁移后仍手写的边缘判断

以下位置都在最终基点复核。它们不是“Kernel 定律已经生成的全部业务规则”，也不能通过把它们写入 Kernel 来消除边缘职责。Session/授权表列规则所有者；纯转发适配器不重复列。

| 类别 | 位置 | 仍由该边缘决定什么；为什么不能从 Kernel 定律推出 |
| --- | --- | --- |
| Session | `R/application/run_access/conversations.rs:15,:63` | Run 事实到 Turn 完成/失败/停止的映射，assistant Done 不结束整个 Turn；current assistant、pending recovery 和 inbound 决定收尾。Kernel 不知道 Turn 产品语义。 |
| Session | `R/application/run_access/conversations.rs:212` | 新 Turn/删除前检查 wait 子闭包；paused/budget/pending 仍占用交接，缺失 child 保守保留。Run 静止复用门面，Session 占用由 Session 决定。 |
| Session | `R/application/run_access/conversations.rs:283`；`R/application/conversations.rs:263,:317` | 前驱历史/工作区继承与删除保留；current assistant 先 retire；缺 admission 不推断已完成。Kernel 不拥有会话引用和继承完成证据。 |
| Session | `R/application/conversations.rs:91,:337,:363,:375`；`R/application/run_access.rs:315` | predecessor 链、被后继替代、wait 祖先作用域、同 Graph/Session 执行互斥；detach 可独立继续。不是重新定义 Run phase。 |
| Session | `R/application/assistants.rs:16,:40`；`R/application/run_access/graph_policy.rs:14` | current assistant 指针/retire/重试准入，身份和 Session eligibility；SessionStore 持有指针权威。Kernel 不定义“当前助手”。 |
| Session | `R/application/run_access/session_calls.rs:8,:21,:69,:95,:117,:228` | continuation 的 budget hold、当前链与 delivery pending；stop/yield 走 Kernel，但 delivered/failed、幂等冲突是 Session 投递事实。不能用 Run Done 冒充投递成功。 |
| Session | `R/application/run_access/channels.rs:78,:101,:248` | 渠道回复失败、启动收尾、进度流闭合；budget 可闭合进度但 paused/pending 保留，错误/缺失按展示策略处理。不是调度授权。 |
| Session | `R/api/wecom.rs:591,:621,:625` | 已替代/Interrupted inbound 不投递，reply 需匹配 Turn/inbound且未 suppressed。Kernel 接受节点结果不代表可向当前用户发送旧回复。 |
| Session | `R/application/run_access/assistant_policy.rs:20,:51` | frontier/已接受结果的 Turn binding 与首轮输入继承；这些是 Session 绑定读取，非可写执行 cursor。 |
| 授权 | `R/application/run_access.rs:338`；`R/application/run_access/projection.rs:67` | frozen Graph digest 与 metadata 身份必须匹配，建议动作按身份/current/superseded/abandon intent 收窄；写前仍验证。Kernel principal 不包含这些 Host 身份绑定。 |
| 授权 | `R/assistant_plugins.rs:248`；`T/graph/logic.rs:663` | invocation authority 必须来自核验的 assistant Host fact；普通 Run 保持 frozen admission；Plugin 声明和当前安装的同授权检查不能由抽象 Kernel 自行发现。 |
| 授权 | `R/execution.rs:150`；`G/plugin_authorization.rs:12` | Plugin resolver 完整性、digest、权限一致与配置。属于宿主授权/Library 契约，Kernel 不解释外部配置。 |
| 授权 | `R/execution.rs:439,:615` | Session-call 收件方与祖先限制、重试时的 frozen 目标，child parent/node/invocation/mode 身份。Kernel 不知道宿主会话/资源作用域。 |
| 执行端口 | `R/application/run_access/kernel_runs.rs:184,:202` | worker 排空、同步注册与删除串行；内存 registry 是实际执行观察，不能从持久 phase 推出线程已退出。 |
| 执行端口 | `R/application/run_access/kernel_runs/pause.rs:43,:90`；`R/application/abandon.rs:111` | 持久 pause/abandon 命令、消费边界、幂等和返回前排空。Kernel 不拥有线程/sandbox 生命周期。 |
| 执行端口 | `R/node_host/facts.rs:16`；`R/node_host/execution.rs:178,:232`；`R/goose_acp.rs:255,:381,:407` | started/completion、来源冲突、旧状态、传输结果和防重放；外部效果事实必须由端口观察，不能从 phase 猜测。 |
| 执行端口 | `R/goose_acp/recovery.rs:6,:34`；`R/application/run_access/kernel_runs/recovery.rs:13,:90,:153`；`R/application/run_access/recovery.rs:70` | 原生 attempt、人工说明目标与冲突、先持久再控制、显式核查接续；Kernel 只裁决提交，不能观察工具现场。 |
| 执行端口 | `R/node_host/execution.rs:292`；`R/goose_acp/transport.rs:678,:691` | wall time、取消与 I/O 中断。时间目前是边缘观察，不是 Kernel 事实；时间是否入核待决。 |
| 执行端口 | `G/graph_call.rs:319,:535`；`R/application/run_access/kernel_calls.rs:241,:310,:333`；`kernel_runs.rs:615` | child drive/停放、Call 结果与产物投影、detached 启动、父绑定；平台与独立 Host 都调用同一个 scheduler，不复刻结算规则。 |

## 边界结论

Run phase/outcome 的唯一持久权威是 Kernel；Host 的生命周期视图、HTTP 投影、Probe 和恢复数组均由事实重建，GraphRunRecord 不再保存第二份 phase/error/recovery/updated。GraphValidator 仍拥有 Graph 路由、模块、InvocationKey 与输入/产物合法性；Kernel 不应替它解释 Graph。SessionStore/metadata 的 current assistant、Turn、delivery、lineage，以及执行端口的工具事实是不同对象，不应为追求数字清零而并入 Run phase。

本次只读审计没有修改这些规则，也未增补 provider/外部投递证据；“没有发现第二份 Run writer”与“已经证明所有并发交错、长期恢复和对抗场景”是不同结论。M3 详细事实归属见 [rules-session-audit.md](../m3/rules-session-audit.md)，M4 字段收尾见 [field-disposition.md](../m4/field-disposition.md)。本表替代引用旧数字时必须保留的口径说明：旧 JSON/扫描漏项、abandon 命令、当前 dormant waiting 分支与 Web 显示公式均已公开登记。
