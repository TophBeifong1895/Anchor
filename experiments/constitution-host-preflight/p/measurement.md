# P：串行 Kernel 路径的 Host 生命周期测量

日期：2026-10-17。旧侧基线 `a234cb6`（H2.1 完成）；新侧为 `ff09f2c feat(host): kernel-path probe for serial Runs (P)`。这是概念探针的源码盘点，运行验收见 [verification.md](verification.md)。默认入口仍走旧 Runner，只有 `POST /trigger` 显式携带 `"execution":"kernel_probe"` 选择新路径。**实际删除旧 Runner / 旧生命周期实现：0 行**；两类 Run 按存储归属路由，未双写同一个 Run。

## 计数口径与可复核边界

执行 `python3 experiments/constitution-host-preflight/p/measure.py` 可重建 [counts.json](counts.json)。脚本从 Git 固定读取旧侧 `a234cb6` 与新侧 `ff09f2c`；结果保存每一项的文件、行、列、类别和原文。`SCOPES` 给出包含端点的源码范围。下文的短名均相对仓库根目录：

| 短名 | 文件 |
| --- | --- |
| R / P | `rust/anchor-runtime/src/graph/runner.rs` / `parallel.rs` |
| S | `rust/anchor-runtime/src/graph/scheduler.rs` |
| K | `rust/anchor-runner-host/src/application/kernel_runs.rs` |
| C / A / U / V | `rust/anchor-runner-host/src/application/control.rs` / `abandon.rs` / `recovery.rs` / `run_projection.rs` |
| E / Q | `rust/anchor-runner-host/src/execution.rs` / `application/queries.rs` |

“手写判断数”采用可重复的语法点数 **I + M + W**：每个 `if`（包括 if-let、else-if、match guard、表达式 if）计 I=1，每个 `=>` match arm 计 M=1，每处直接字段赋值（包括 `+=`）计 W=1。去掉注释与字符串后计数；条件中的多个 `&&` 不拆分，`?`、`let … else`、集合的 insert/retain 等方法调用、结构体初始化、布尔字段表达式不计。这不是圈复杂度，也不是完整业务规则数；相同口径同时覆盖新侧的 Phase、Reason、completion 和 Outcome 分支，连只读投影的两个字段赋值也计入 W。停止/暂停轮询是 I 的子集，不重复加分。

旧侧取现有操作入口及其共享判断的**代码包络**，没有假装把嵌在函数里的 Session、Call、并行和助手能力都迁完：例如 R:31–765 包含这些旧能力，新侧遇到它们则具名 Unsupported。因此表中差值同时包含生命周期规则下沉和探针范围缩小，不能当作同等能力的净收益。stop/resume/abandon 共用的 K:135–172 和 S:32–52 在各行重复列入，重启也复用执行循环；行之间不可直接相加。最后的 union 按源位置去重。

“新增胶水行数”是新侧所列范围中，相对基线新增的非空、非纯注释源码行；已有共用的 `parse_reason` 不算新增。测试、fixture 搬移、依赖声明不计入胶水。执行循环、控制、重启、投影以外的存储选择和准入适配另列在后文，不藏在表内。

## 按操作对照

括号依次为 I/M/W；所有范围对应 `counts.json` 的 scopes 与逐点 hits，旧范围均指 `a234cb6`。

| 操作 | 旧路径手写判断数与位置 | 内核路径手写判断数与位置 | 被内核定律取代的判断 | 新增胶水行数与位置 |
| --- | --- | --- | --- | --- |
| 执行循环 | **103**（45/27/31）；R:31–765、961–981；E:283–305 | **22**（7/14/1）；S:14–24、32–52、64–190；K:29–38、108–134、303–320 | ①执行/等待/终态写入；②终态不可再提交；③等待拒收节点结果；④完成与节点失败唯一结算；⑤成本由裁决历史累计；路由、计数、轮次上限和结果并入则由 **GraphValidator** 接管，属于 Graph 规则而非 Kernel 定律 | **214**；同新侧范围 |
| stop | **40**（25/6/9）；C:35–171、220–243；R:68–77、258–262、371–378、581–590；P:66–76 | **7**（6/0/1）；C:32–34；K:135–172；S:32–52 | ①停止写 Waiting(Human)；②终态拒绝控制；③在跑结果迟到以 TaskWaiting 拒绝并保留历史；取消信号仍由 Host 在接受后触发 | **62**；同新侧范围 |
| resume | **38**（27/6/5）；C:35–195、208–252；U:159–176；R:31–67 | **10**（9/0/1）；C:32–34；K:135–172、321–354；S:32–52 | ①只有合法等待能释放；②Human 等待不能由 Code 释放；③已放弃/结算不可恢复；④预算结算不能被普通 resume 翻转；活跃 worker 排他仍由 Host 判断 | **96**；同新侧范围 |
| abandon | **25**（10/13/2）；A:72–82、124–317 | **11**（7/3/1）；A:72–82、191–199；K:135–172；S:32–52 | ①直接结算 Aborted；②结算不可逆；③后续结果与控制被拒；无需旧侧 abandon intent 与 Run 终态之间的补写阶段。旧请求幂等回包语义没有搬入本探针 | **68**；上述新侧范围，扣除已有 A:72–82 |
| 未知效果 / recovery | **24**（18/1/5）；U:6–231；C:173–195；R:603–614 | **7**（2/5/0）；U:14–16；S:83–96、157–165、192–218 | ①Code 只能凭 Observation stop；②等待人后不接普通结果；③等待的释放权归 Human。**如何核查外部效果仍属执行端口，旧 attempt decision API 暂未迁移，不能把这一部分差值都归功于定律** | **53**；同新侧范围 |
| 重启 | **24**（17/2/5）；R:31–67、102–266、303–319 | **11**（6/5/0）；S:64–119；K:59–93、108–134、321–354 | ①恢复时读持久相位；②结算后不再派发；③已接受结果对应空位不再开放；开放空位由 GraphValidator 推导，completion_fact 保证同 invocation 的恢复依据。文件锁与 worker 排他仍是 Host 责任 | **151**；同新侧范围 |
| 状态投影 | **16**（9/4/3）；V:28–108；C:220–252 | **14**（5/7/2）；V:33–39；K:173–233；Q:49–82 | ①终态直接读 is_settled；②等待直接读 Phase；③前端动作不再调用旧 RunStatus 控制校验器。相位到旧状态的映射、活跃/排空与 updated 时间查询仍需适配 | **87**；同新侧范围 |

去重后：旧侧 **193 个语法点、1416 个源码行**，新侧 **51 个语法点、472 个源码行**，其中相对基线新增 **449 行胶水**。数字与范围见 `counts.json → union`；已有23行不计新增。表外适配包括 K:235–299 的缓存/归属/列表读取、K:40–58 的锁与登记释放、`application/admission.rs:10–92` 的显式准入，以及 `node_host.rs` 和其消费者的统一 Run 读取；新路径不会绕过原 NodeExecutionRequest 授权构造。故 449 不能代表整个提交的生产代码增量。

轮询专项复核：旧 R:68、73、258、373、581、586 共6处读取现场；另 P:66 是两个标志的组合读取、P:67 再读 stop，总计 **8处条件 / 9次标志读取**。任务书的“6处”对应 R 内的起点；没有忽略 parallel 的额外读点。新 S 与 K 的 `stop_requested` / `pause_requested` 均为 **0**。`cancellation.store` 是内核接受 stop/abandon 后对执行器发出的取消，不是调度循环的轮询。

## 三个问题

仍需手写的是授权身份、Graph 能力范围、唯一存储归属、进程写锁与同 Run worker 排他、执行请求/输入/产物适配、completion_fact 分流、迟到和版本过期的客户端处理、API 投影及错误映射。它们分别属于 Host 授权、执行端口和入口适配，不应全塞进 Kernel。Session/Turn、子调用、渠道投递与常驻助手的判断仍在旧路径，P 没有验证它们会消失；未知效果的核查也是 Goose/外部系统的事实，Kernel 只保存观察并守住等待和释放权限。这里没有发现必须新增 Kernel 定律的场景，也没有新增恢复引擎；但人核查到真实 Goose completion 的接口尚未接通。

若假设后续所有旧操作都能达到本探针的行数比例，472/1416≈0.33，测量包络的1416行可由约472行适配替换（约449行为新增）；考虑表外归属/准入/读取胶水，先以新增 **600–800行**估计这一包络。再把同一比例粗放外推到 R、P、C、A、U 的生产源码共 **2141行**（分别946/464/232/290/209非空非注释行），对应可替换约 **2100行**、新增约 **700–1100行**，理论净减约1000–1400行。**这只是乐观容量估算，不是已获证的可删清单**：旧包络含未迁的Session、Call、助手、失败恢复能力，也含应保留的端口与授权逻辑；它们会使实际可删量下降、适配量增加。P 的真实结果仍是旧实现删除0，新增一条可运行串行路径，尚不足以批准全面删除 Runner。

最别扭的是兼容旧查询模型：Kernel 已有权威 Phase，现有消费者却要求 GraphRunRecord.status 和旧文件 updated 时间。K:173–233 必须复制并投影状态，Q:59–76 甚至为了取得 FileStore 的 mtime 复用了其文件名编码规则；这让语义写入收敛后，读适配仍有耦合。复制从不回写，也拒绝同一 ID 两份存储，守住了单一权威，但这不是理想的最终查询契约。后续应先定统一 Run 读模型/时间来源，再决定是否扩展 Store 公开观察接口；P 没有为此修改 Kernel。

## 未决

| 现场 | 理由 | 影响 |
| --- | --- | --- |
| fanout/join、Op.call、host/常驻助手、Yielded/Interrupted/Aborted/BudgetExhausted 执行结果 | 本刀只验串行 Work；后几类结果还需宿主确认成本与控制事实 | 返回带 P 场景名称的 Unsupported；旧路径继续承担完整能力 |
| Kernel pause、旧 Goose attempt recovery decision、Run/Graph 删除 | pause采用stop/resume；attempt恢复与删除需新的事实归属接线 | 现有入口明确拒绝。fixture 可核查后把 completion_fact 改为已完成再resume；真实Goose这一路未验，Uncertain不消除则再次等待 |
| 原生Kernel相位映射为旧RunStatus；updated依赖FileStore布局 | 保持既有API消费者可读；Kernel当前没有公开文件时间接口 | 只读投影不保存；后续统一读模型前保留适配耦合 |
| `<state>/kernel-runs` 单writer flock；draining时拒绝resume | FileStore自身不是多进程事务协调器；防止同空位两个worker执行 | 不支持多个Host进程共享此探针目录；在跑提交可迟到但不可同时重启执行 |
| 显式 `execution=kernel_probe` | 保留旧测试和默认产品路径，同时让同一应用入口可验新路径 | 非迁移开关，不迁旧Run、不产生第二份权威 |

以上属于任务书允许自行登记的 Host 接口与未支持边界；没有触发修改 Kernel、校验器生产裁决、弱化断言或自动重放未知效果的停止条件。
