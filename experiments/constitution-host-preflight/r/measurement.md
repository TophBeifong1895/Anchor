# R：统一 Run 读模型测量

2026-10-17。基线 `1d1ba54`，R0 `cb2219c`，读模型实现 `54e664e`。结论：**需要重想**。所选消费者连同保留下来的适配规则，I/M/W 判断点由 **82 增至 90**；没有达到减少至少 50% 的门槛。四个消费者的生产 `RunStatus` 引用为 0，只证明接口隔离，不能证明规则消失。H3–H6 不据此扩大。

## 口径与复现

沿用 [P 的 measure.py](../p/measure.py)：I 为 `if`（含 if-let、else-if、表达式和 match guard）；M 为 `=>`；W 为直接字段赋值。排除注释、字符串、测试文件及 `#[cfg(test)]` 项。普通布尔表达式、`matches!`、`is_terminal()` 调用本身不产生 I/M/W 点；所以 liveness 为 0 不表示没有判断。下表同时逐条说明这些布尔规则，不用标识符数量冒充分支数量。

范围由 [measure.py](measure.py) 的函数名定义，自动定位完整函数；包括其中原有的 Session、身份、Graph 和交接条件，而非只摘变化最大的几行。所有旧/新命中、源码行、范围和新增行见 [counts.json](counts.json)。把移入临时适配层的规则计回相应行；共享适配另加一次，避免移动制造收益。新增胶水是相对基线的新增非空、非注释源码行，**包含搬入新文件的旧规则**，不是净增业务规则数。

从仓库根目录复现：

```bash
PYTHONDONTWRITEBYTECODE=1 python3 experiments/constitution-host-preflight/r/measure.py 54e664e
PYTHONDONTWRITEBYTECODE=1 python3 experiments/constitution-host-preflight/r/lock_sites.py cb2219c
```

输出：[measure.log](measure.log)、[lock-sites.log](lock-sites.log)、[lock-sites.json](lock-sites.json)。脚本只读取 Git 对象并写本目录产物，不依赖运行中的 Host 或 Provider。

## 消费者逐项

表内的“新消费者 + 保留适配”区分源码位置，右侧总数才用于前后比较。源码缩写：V=[run_projection.rs](../../../rust/anchor-runner-host/src/application/run_projection.rs)，L=[liveness.rs](../../../rust/anchor-runner-host/src/application/liveness.rs)，C=[conversations.rs](../../../rust/anchor-runner-host/src/application/conversations.rs)，K=[kernel_runs.rs](../../../rust/anchor-runner-host/src/application/kernel_runs.rs)，A=[legacy_run_view.rs](../../../rust/anchor-runner-host/src/application/legacy_run_view.rs)，J=[run_json.rs](../../../rust/anchor-runner-host/src/application/run_json.rs)。旧位置指基线，新的位置指 `54e664e`。

| 消费者 | 改前 I/M/W | 改后 I/M/W；新消费者 + 保留适配 | 被相位或内核定律取代的 | 保留的判断及归属 | 新增胶水行 |
| --- | --- | --- | --- | --- | --- |
| run_projection | **13**（6/4/3）；V:28–115 | **13**（6/4/3）= V **8** + A **5**；V:28–85、A:71–114 | 原 `status.is_terminal()` 的取值改用 `view.is_settled()`；没有删除对应的兼容分支 | ①身份、当前助手、被取代会话、abandon intent、session_call：授权/Session，V:51–67。②worker 的暂停/停止请求、孤立 Running 展示、inactive stop/resume、预算和旧 recovery：旧产品行为，A:79–111；**不是 Session 条件，Phase 缺少这些旧区别** | **50**；同新范围 |
| liveness | **0**（0/0/0）；L:16–35 | **0**（0/0/0）；L:8–22、A:54–69 | 原生 Run 以 `!worker && (Settled 或 Waiting(Human))` 推导静止，L:10–17 | ①worker：宿主观察。②旧路径只有终态/Stopped 且无 worker 才静止，A:54–56；Paused、BudgetStopped 与 Stopped 同映为 Waiting(Human)，旧静止结果却不同，**Phase 缺少旧行为信息**；原断言保留 | **24**；同新范围 |
| conversations | **59**（31/21/7）；C:115–219、386–431、579–631、671–862 | **58**（33/18/7）= C **53** + A **5**；C:115–203、370–415、563–615、655–857、A:117–133 | ①后继是否终态；②两类 scope 遍历排除终态；③admit 排除终态，均读 `is_settled()`。④Ready/Running busy 改读 Executing。⑤静止读取经统一查询。它们改变取值来源，**并未消除 if** | ①Session 当前 turn、助手身份、scope、交接链、租约及投递条件均保留在 C。②两个旧状态到 TurnStatus 的 match 合为 A:127–131 的四臂；Completed 在助手分支不结算的既有条件也保留。③完成后产物分支改读既有 TurnStatus。把已存在的 Session 状态作为输入，未新增状态枚举；多出的读模型查询 guard 也计数 | **37**；同新范围 |
| kernel_runs 投影 | **10**（1/7/2）；K:173–230 | **10**（1/7/2）= K **0** + J **10**；K:183–204、J:1–74 | ①直接用 Kernel Phase 构造 RunView。②stop/resume/abandon 在 RunView 中按三条定律推导。③终态取 `is_settled()`。这些布尔式没有被 I/M/W 计为“删除” | ①Phase 到旧 status/error 的七臂转换、两次副本赋值搬到 J:9–41。②无 worker 的 Executing 仍显示 waiting_resume 并允许显式启动 driver；JSON quiescent 仍为 !worker，J:44–73。均为 **P 的既有 API 兼容语义**，不属于 Session/授权，不能算删掉 | **84**；同新范围 |

四行合计 **82 → 81**。另外，共享的旧状态到 Phase 适配七臂（A:10–45）和查询归属的两个 if（[queries.rs:145–172](../../../rust/anchor-runner-host/src/application/queries.rs#L145)）共 **9** 点；RunView 本身的动作/终态布尔式按此口径是 0。新侧去重合计 **90**（42 I / 36 M / 12 W），旧侧 **82**（38 I / 32 M / 12 W）。完整包络非空、非注释源码 **545 → 726 行**，新增胶水 **306 行**（四行195 + 共享111）。时间观察实现不在这组消费者比较里，另列成本。

如果只看四个原文件，忽略移出的规则，会得到 **82 → 61**（减少25.6%）；这仍达不到50%，更不能作为实际简化结论。连同规则保留位置计算，90/82≈1.098，即约增加9.8%。

**统一输入的验收边界**：原生消费者已可直接读RunView；旧projection仍在临时适配中同时读取原record，旧quiescence也保留RunStatus入口。因此本刀交付的是可运行的原生读模型和旧规则隔离探针，**没有把“所有消费者只靠RunView一种输入”记为完成**。Paused与Stopped在视图中同为Waiting(Human)、无worker，旧静止断言却要求false/true；Ready与孤立Running同为Executing，旧JSON却要求不同phase。继续消除原record依赖需要审查旧契约，不能通过新增状态替身悄悄补齐。

## 单列的适配与观察成本

这些范围可能与上表重叠，**不再加一次到90上**。旧适配的行数没有被算作消费者删除收益。

| 实现 | 非空非注释行 / 物理行 | I/M/W 点 | 归属与限制 |
| --- | --- | --- | --- |
| 临时旧 Run 适配 A:1–134 | **128 / 134** | **17**（2/15/0） | 状态到 Phase、旧 quiescence、旧 JSON/action 及旧 Session callback；标明迁移后删除。只统计 `phase/view` 会掩盖其余旧规则，因此整文件单列 |
| JSON 边界 J:1–74 | **67 / 74** | **10**（1/7/2） | 原生 Phase 到既有 status/error/projection；兼容副本永不保存 |
| [宿主时间观察](../../../rust/anchor-runner-host/src/application/run_observations.rs):1–57 | **55 / 57** | **4**（1/3/0） | Host 的 `kernel-run-observations/{id}.json`，带序列化写入与原子替换；只存 observed updated，不是 Kernel 事实 |

时间接线还涉及 `queries::run_updated`、KernelRuns 创建初始化、Scheduler 的 optional SubmissionObserver。`submit_with_retry` 仅在接受后通知观察者；拒绝、迟到和 stale 重试不盖新的接受观察。观察读写失败不撤销已接受事实，也不影响控制或生命周期读取。具体范围及其完整函数成本在 `counts.json → separate_costs`；这些函数包含原有逻辑，不全是新增代码。**没有读取内核路径编码、文件布局或 mtime，也没有给 Stamp 加时间。**

## R0 锁站点

[lock-sites.json](lock-sites.json) 逐条列出基线 **22 个低层获取站点、6 个显式释放站点**。改后低层实现只有共享 crate 内 **1 个获取入口 + 1 个 Drop 显式释放**；**22 个调用站点继续存在**，分别保留 blocking/nonblocking、shared/exclusive 与可取消重试策略，没有把调用者删除算成收益。`FileLease::release` 也调用同一底层入口，保留 devtools 原来检查释放错误的行为。

没有保留未改的生产Rust低层站点。另有[tool_environment.rs:327](../../../rust/anchor-runner-host/src/tool_environment.rs#L327)中嵌入shell的`/usr/bin/flock -x 9`保留：它在沙箱的Cargo缓存初始化subshell内打开并持锁，Host没有对应Rust File租约；其描述符生命周期属于该subshell，不改为Host提前解锁。脚本将此处作为单独的保留项记录，不混入22个Rust获取站点。`run_data` 的提前返回、panic/unwind 和 fork 继承描述符均通过同一个 Drop 覆盖；测试用同一 open file description 的复制描述符，验证提前返回/展开后另一独立描述符可获得锁。失败争用不会释放其他租约；共享读锁仍可并存。这里验证的是描述符继承所依赖的 OS 行为，没有把这些单测冒充真实 Provider 端到端验收。

R0 已单独同步 master：`6394e28`；保留 master 原有 Runtime shared read lease 和 credential lease 可见性，没有带入该分支的 Kernel 探针。master 的相关 all-target check 与132项定向测试通过，见 [verification.md](verification.md)。

## 三个问题

**1. 值得迁移还是需要重想？** 需要重想。完整同口径范围82→90；run_projection和liveness的旧规则只是移进适配，Session例子也只净少一个语法点。剩余判断不能全归属 Session/授权：Ready/Running 的展示与启动、Paused/Stopped/BudgetStopped 的静止差异、旧 recovery 和预算都超出了这三类 Phase 所表达的信息。写侧定律仍成立；本刀没有证明在保持旧 JSON/行为时读侧会自然简化。下一步应先审查哪些旧差异继续作为产品契约、哪些应显式改约，不能据此全面搬迁或删除旧 Runner。

**2. 剩余文件能删多少、哪些最难？** 精确盘点与任务书的约数不同：同样排除测试、注释和字符串后，基线为 **154 个 `RunStatus` 标识符 / 22 文件**；本刀为 **151 / 20 文件**。所选四文件由33→0，其余18个原文件仍有121个标识符，新临时适配21、新JSON边界9。这18个文件的既有RunStatus读写规则未因R改变（application.rs只增加模块声明，R0另改锁）。按本刀真实比例外推，**没有可据此批准的分支删除量**；若机械套用90/82，待迁判断包络还可能增加约10%，而不是减少50%。标识符不是分支，不能把121乘比例当可删规则数。各文件如下；最难的是同时写旧状态并处理 Call/恢复/Session 的几处。

| 剩余原文件（相对 runner-host/src） | 生产标识符数 | 预估边界 |
| --- | --- | --- |
| api.rs | 12 | 旧状态字符串表；可移至边界，不能视为规则删除 |
| api/files.rs | 1 | 只有完成 Run 才可读取特定结果，仍需保留产物约束 |
| api/turns.rs | 5 | Session turn 终结/停止与绑定条件，需保留 Session 策略 |
| api/wecom.rs | 16 | **难**：完成/失败/停止消息、孤立运行与投递，跨 Run/渠道事实 |
| api/wecom_progress.rs | 5 | 渠道流观察与等待展示，不是通用控制定律 |
| application.rs | 1 | 共用 import，不是可删分支 |
| application/abandon.rs | 16 | **最难之一**：持久 intent、Call 关闭、租约与幂等结算，包含写状态 |
| application/admission.rs | 8 | 未结束 Run 的准入及单一执行范围，需验证授权/排他条件 |
| application/assistants.rs | 1 | 助手退役是否已停止，与 Session 交接相关 |
| application/control.rs | 12 | **最难之一**：父子 Call 停止、预算、recovery 与控制写入 |
| application/deletion.rs | 2 | 删除要求静止与租约；不能仅按相位放行 |
| application/dispatch.rs | 10 | **难**：Ready 调度、父子结算及等待释放，不是状态别名替换 |
| application/graphs.rs | 2 | Graph 活跃 Run 与生命周期展示 |
| application/recovery.rs | 7 | **最难之一**：未知效果与 Goose 恢复事实；包括旧状态写入 |
| application/session_calls.rs | 11 | **最难之一**：长期等待、父子 Run、投递与 Session 恢复；包括状态写入 |
| execution.rs | 5 | 执行失败落盘与 callback，需要保留原后端事实与错误边界 |
| main.rs | 2 | import 与 callback 类型声明，不是分支 |
| node_host/conversation.rs | 5 | 可继续会话的结局筛选，还需 Goose/Session 核查 |

完整行号、源码和逐文件数量见 `counts.json → status_inventory`。四个所选文件的原始搜索输出保存在 [runstatus-grep.txt](runstatus-grep.txt)：V有15行、L有11行，全部位于原有/新增 `cfg(test)`；C和K均0行。**生产四文件各0，测试断言未删除。**

**3. RunView 有没有偷偷重造 RunStatus？** 没有新枚举、旧状态位组或完整旧记录藏在视图里：只有 Kernel Phase、Graph 的 passes、worker 和可缺省的 observed updated；actions由三条规则派生。passes是实际供详情查询使用的Graph进度，不能重建旧状态。可是“没有重造”并不等于已经统一：Phase不能无损表示旧Ready/Running和Paused/Stopped的区别，所以边界仍读取旧记录以保留字节和行为。这是负结论的证据，不能用一个一一对应的新enum或改名字段掩盖。旧Failed没有Kernel原因码，临时适配明确用现有 `Reason::Unclassified`，不编造ContentInvalid；原错误detail和对外JSON保留，未写回新原因码。

## 未覆盖与交回决定

- 未触发需要改Kernel、弱化断言、加ignore、改变旧Run JSON或改Web的硬停止条件；未新增与RunStatus一一对应的状态枚举。
- 旧API保持字段/取值；有Ready/Paused/Stopped三组精确序列化字节回归，并保留原10状态动作和全部静止断言。这不声称穷举了所有HTTP对象的逐字节组合。
- 既有P原生Run若没有宿主观察文件，不伪造历史时间；更新时间查询明确报观察不可用。之后接受的提交可建立新观察。观察损坏不妨碍控制或RunView生命周期读取；该缺失边界尚无HTTP恢复迁移验收。
- 未迁P的不支持能力；真实Goose/Provider、Web和部署未新增验收。原有差分组与P的1000 seed/真实Op.run回归在最终全量测试中通过，见[验证证据](verification.md)。
- 审查方需决定旧展示/动作差异是否继续作为产品契约，或授权显式改约后再做一刀同能力比较。R0可独立使用；R是有负结论的探针交付，不能作为H3–H6全量迁移的批准。
