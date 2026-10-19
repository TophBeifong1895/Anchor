# M1.5 语义规则清单

比较 `1d1ba54`（R 之前）与合并 `master@2b3b7a2` 后的工作树。结论是：旧/原生读取共用动作与静止定义有所收敛，但旧控制、子调用和恢复编排仍各有独立判断；M2–M3 尚未执行，不能宣称 Run 规则已全面统一。全部位点、原句与源文件摘要见 [counts.json](counts.json)。

## 计数方法

- 单位是自主写出的规则定义或前置条件。调用同一个 `is_terminal`、`RunLiveness`、`view.actions()` 或派生结果，不重复计；同一 helper 中多条不同约束分别列，多个地方各自写 worker/phase 门禁也分别列。每行的 before/after 按相同方法审阅。
- Host 仅指 `anchor-runner-host`、`anchor-graph-host`。runtime、Kernel 与 Web 分栏列出，均不混入 Host 数；测试、fixture、纯序列化、状态字段赋值本身不计。`legacy_run_view` 把旧生命周期解释成新的相位，属于必须显式保留的语义适配。
- 组合规则不反向展开它依赖的其他规则。例如 `reject_terminal_control` 是控制前置条件，调用 runtime 终态谓词时不另算一个终态定义；abandon 的终态/幂等细分放在 abandon 行。各行不可相加为全局独立实现总数，同一门禁可能参与多条规则。
- Session 的 Turn/当前实例/投递，授权、身份、产物可见性等独立策略单列。它们读取 Run 事实并不自动成为 Run 生命周期规则；这也不等于其事实所有权已全部验清，M3 仍须按任务书 §10.5 复核。
- 位点由语义审阅确认，`rg` 只用于查找入口，不把命中数当规则数。`counts.json` 保存 baseline 完整提交、合并输入、每个位点原句和文件 SHA256；after 行号对应该工作树快照。原始搜索在 `/tmp/anchor-m15/rules-1d1ba54.txt`、`rules-WORKTREE.txt`；本表的临时生成/复核脚本在 `/tmp/anchor-m15/build-rule-inventory.py`，这些临时材料不入库。

## Host 规则数量

| 规则 | `1d1ba54` | 合并后 | 结论 |
| --- | ---: | ---: | --- |
| 是否终态 | 1 | 3 | 旧准入显式状态集合仍在；新增 View 谓词和旧状态适配。 |
| 可否 stop | 4 | 3 | 两个读取建议合为一个；旧写入门禁仍在。 |
| 可否 resume | 12 | 11 | 两个读取建议合为一个；旧恢复/预算及原生重复 worker 检查仍在。 |
| 可否 abandon | 3 | 4 | 旧写入与竞态复核保留，新增共享动作建议。 |
| 可否 pause | 5 | 4 | 删除 probe 两处 Unsupported 规则，新增原生 pause 校验；旧写入门禁仍在。 |
| 是否静止 | 2 | 1 | Host 合为一个；Web 另有同义公式。 |
| 是否需要关注 | 2 | 0 | 规则迁至 Web 一处，不能当作产品中没有这条规则。 |
| 迟到结果如何处理 | 1 | 1 | Host 保留父子回调门禁；普通节点晚到裁决在 Kernel。 |
| 未知效果如何处理 | 13 | 14 | 旧恢复与执行端边界保留；新增 Effect wait 的解释。 |
| 预算耗尽如何处理 | 3 | 4 | 旧分层规则保留；新增 Human wait 的解释；原生执行结果仍 Unsupported。 |
| 子 Run 结算后如何释放父 Run | 3 | 3 | 仍是旧调用路径；原生 Child wait 未接入。 |

## 逐规则位点

### 是否终态（1 → 3）

只统计终态/未终态的通用分类定义；按结果细分的 abandon、子调用和 Session 投递策略在各自规则下登记。RunStatus::is_terminal 与 Resource::is_settled 的调用不计。

两侧相同：

- `rust/anchor-runner-host/src/application/admission.rs:64`：显式列举旧 Run 的全部未结算 status，决定是否仍占用 Graph 准入；不是调用 is_terminal。

仅合并后：

- `rust/anchor-runner-host/src/application/legacy_run_view.rs:36`：旧 status 到相位的唯一适配定义；Completed/Failed/Aborted 被解释为 Settled。
- `rust/anchor-runner-host/src/application/run_view.rs:50`：原生 View 的终态谓词；消费此谓词的 abandon/actions 不再计一处终态定义。

runtime（合并后，Host 数之外）：

- `rust/anchor-runtime/src/graph/state.rs:59`：旧 Run 的唯一通用终态谓词。

Kernel（合并后，Host 数之外）：

- `rust/anchor-kernel/src/kernel.rs:227`：结算不可逆；提交由 Kernel 拒绝。

### 可否 stop（4 → 3）

建议动作与旧控制写入门禁是不同独立判断。Kernel 控制写入不在 Host 再手写 phase 门禁，委托 Kernel；身份、scope 和 Session 协调收窄另列。

两侧相同：

- `rust/anchor-runner-host/src/application/control.rs:87`：旧 worker 已注册时只允许 pause/stop；resume 被拒绝。
- `rust/anchor-runner-host/src/application/control.rs:224`：旧控制的统一终态门禁；各调用点不重复计。

仅 baseline：

- `rust/anchor-runner-host/src/application/run_projection.rs:108`：旧 active Run 的 stop 建议值还依赖请求标志。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:226`：原生 probe 独立定义 Executing 可 stop。

仅合并后：

- `rust/anchor-runner-host/src/application/run_view.rs:37`：共享 View 的 stop 建议值；旧/原生读取共用。

Kernel（合并后，Host 数之外）：

- `rust/anchor-kernel/src/kernel.rs:504`：Human 或带 Observation 的 Code 可提交 stop；结算门禁在 adjudicate。

### 可否 resume（12 → 11）

共享 helper 的每次调用不计；多个位置各自用 worker/phase 决定 resume，仍分别计。Session 当前实例、superseded Turn、abandon intent 的单向性另列边界，未混入此处。

两侧相同：

- `rust/anchor-runner-host/src/application/control.rs:87`：旧 worker 已注册时只允许 pause/stop；resume 被拒绝。
- `rust/anchor-runner-host/src/application/control.rs:224`：旧控制的统一终态门禁；各调用点不重复计。
- `rust/anchor-runner-host/src/application/control.rs:236`：旧 inactive effect/recovery 状态不能直接续跑，先走恢复用例。
- `rust/anchor-runner-host/src/application/control.rs:249`：旧 budget-stopped 需要显式预算策略，普通 resume 拒绝。
- `rust/anchor-runner-host/src/application/control.rs:180`：普通 resume 遇到待核查效果，先为同一 Agent 保存观察上下文。
- `rust/anchor-runner-host/src/application/control.rs:74`：子完成回调只可续跑同一 WaitingCall 节点和 invocation；已活动时不重复调度。
- `rust/anchor-graph-host/src/graph_call.rs:342`：显式枚举可在父调用内重新进入的子 Run 状态。

仅 baseline：

- `rust/anchor-runner-host/src/application/run_projection.rs:90`：旧读取独立组合 worker、恢复上下文与共用门禁产生 resume 建议。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:227`：原生读取独立定义 resume 建议。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:146`：原生控制提交前禁止 worker 未退出时 resume。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:330`：原生续跑用例在准备执行端口前再检查 worker。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:350`：Executing 且无 worker 可直接重进调度；其他相位先提交 resume。

仅合并后：

- `rust/anchor-runner-host/src/application/run_view.rs:38`：共享建议：Executing/Human wait 且无 worker；child/effect 不接受人的普通 resume。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:183`：原生控制提交前禁止 worker 未退出时 resume。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:331`：原生续跑用例在准备执行端口前再检查 worker。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:351`：Executing 且无 worker 可直接重进调度；其他相位先提交 resume。

Kernel（合并后，Host 数之外）：

- `rust/anchor-kernel/src/kernel.rs:522`：谁等待谁释放：Human wait 由人，Child/Effect wait 由 Code。

### 可否 abandon（3 → 4）

旧路径保留取消/竞态保护；原生写入直接委托 Kernel。新增 advisory action 不代表旧写入判断已删除。

两侧相同：

- `rust/anchor-runner-host/src/application/abandon.rs:159`：最终落盘只把仍未结算且 worker 已退出的旧 Run 变 Aborted。
- `rust/anchor-runner-host/src/application/abandon.rs:213`：放弃前置条件：Aborted 幂等，Completed/Failed 拒绝，其余可请求。
- `rust/anchor-runner-host/src/application/abandon.rs:299`：取消后重新检查事实；已完成/失败不可反转为放弃。

仅合并后：

- `rust/anchor-runner-host/src/application/run_view.rs:46`：新增共享 abandon 建议；调用 is_settled，不再重复定义终态。

Kernel（合并后，Host 数之外）：

- `rust/anchor-kernel/src/kernel.rs:538`：Human 可放弃未结算任务；结算任务此前统一拒绝。

### 可否 pause（5 → 4）

原生 pause 的独立意图在派发边界消费，不取消当前节点；边界提交 stop 是动作实现，未再算一处可 pause 判断。旧 Runner 的轮询机制仍待 M3 删除。

两侧相同：

- `rust/anchor-runner-host/src/application/control.rs:87`：旧 worker 已注册时只允许 pause/stop；resume 被拒绝。
- `rust/anchor-runner-host/src/application/control.rs:242`：没有旧 worker 的 Run 不可 pause。

仅 baseline：

- `rust/anchor-runner-host/src/application/run_projection.rs:106`：旧 pause 建议额外要求没有控制请求。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:225`：原生 probe 读取宣布 pause 不支持。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:141`：原生 probe 写入宣布 pause 不支持。

仅合并后：

- `rust/anchor-runner-host/src/application/run_view.rs:36`：共享建议：Executing 且有 worker。
- `rust/anchor-runner-host/src/application/kernel_runs/pause.rs:92`：原生 pause 写入同样校验 worker 和 Executing，然后保存意图。

边缘/动作实现（合并后，Host 数之外）：

- `rust/anchor-runner-host/src/application/kernel_runs/pause.rs:43`：当前结果提交后的边界把 pause 意图转为 reason=paused 的 stop；已等待/结算不改事实。

### 是否静止（2 → 1）

Session 仍可持有 paused/budget Turn；这不是重新定义 Run 静止。Web 仍有相同相位公式的独立实现，跨语言未收敛为一处。

仅 baseline：

- `rust/anchor-runner-host/src/application/liveness.rs:33`：旧静止定义：没有 worker，且旧终态或 Stopped。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:220`：probe 的独立静止定义只看 worker。

仅合并后：

- `rust/anchor-runner-host/src/application/liveness.rs:13`：唯一 Host 定义：没有 worker，且 Settled/Human wait。

边缘/动作实现（合并后，Host 数之外）：

- `apps/web/src/runPresentation.ts:12`：Web 静止公式；Host 计数之外仍存在这一份定义。

### 是否需要关注（2 → 0）

Host 已不提供该事实，Web 由 phase/outcome/active 生成显示规则。2→0 是规则迁出 Host，不是整个产品只剩零处。

仅 baseline：

- `rust/anchor-runner-host/src/application/run_projection.rs:96`：旧状态集合决定是否需关注。
- `rust/anchor-runner-host/src/application/kernel_runs.rs:221`：probe 独立按 waiting/丢失 worker/失败决定关注。

边缘/动作实现（合并后，Host 数之外）：

- `apps/web/src/runPresentation.ts:10`：Web 独立定义关注状态；等待、非 Done 结算、失去 worker 等。

### 迟到结果如何处理（1 → 1）

Host 这一处是父子唤醒回调的防护；一般节点迟到提交由 Kernel 拒绝，Host 不重写相位。Session 的旧回复抑制属于投递规则，另列。

两侧相同：

- `rust/anchor-runner-host/src/application/control.rs:74`：迟到的子 Run 完成回调必须匹配仍等待的父节点与 invocation，且父无 worker。

runtime（合并后，Host 数之外）：

- `rust/anchor-runtime/src/graph/scheduler.rs:55`：StaleVersion 刷新版本重提，重提成本归零。
- `rust/anchor-runtime/src/graph/scheduler.rs:219`：TaskWaiting/TaskSettled 的晚到结果不推进 Graph；其他拒绝报错。

Kernel（合并后，Host 数之外）：

- `rust/anchor-kernel/src/kernel.rs:213`：Waiting 时晚到 work 拒绝，成本仍按既定盖章处理。
- `rust/anchor-kernel/src/kernel.rs:227`：结算不可逆。
- `rust/anchor-kernel/src/kernel.rs:234`：版本不匹配返回 StaleVersion。

### 未知效果如何处理（13 → 14）

本表同时列出未知识别与防重放/人工决定这些不同子规则，未把它们声称为同一份实现。原生 record_recovery_decision 当前仍 Unsupported，是 M2 缺口；Session-call 的观察回写另列。

两侧相同：

- `rust/anchor-runner-host/src/node_host/facts.rs:45`：有 started 无终结结果就是 Uncertain，不自动重放。
- `rust/anchor-runner-host/src/node_host/execution.rs:207`：Host/Goose 结果不一致保留未知。
- `rust/anchor-runner-host/src/goose_acp.rs:378`：fixture 缺终结结果为未知；原生 v2 有原生会话时仍可恢复。
- `rust/anchor-runner-host/src/goose_acp.rs:398`：只允许 NotStarted 或原生 Resumable 进入，未知事实不重放。
- `rust/anchor-runner-host/src/goose_acp.rs:249`：原生传输等未知故障保留 Interrupted，合同错误才是已知失败。
- `rust/anchor-graph-host/src/graph_call.rs:342`：子 Run 的 recovery/terminal 状态不自动重进执行。
- `rust/anchor-graph-host/src/graph_call.rs:643`：子失败但保留 cursor、或仍 Ready/Running：向父暴露 Uncertain。
- `rust/anchor-runner-host/src/application/dispatch.rs:51`：崩溃后 detached Running 可能已逃逸，只自动派发尚 Ready 的 child。
- `rust/anchor-runner-host/src/execution.rs:307`：旧 Runner 异常只基于最新持久记录失败并保留未知现场，不自动回放。
- `rust/anchor-runner-host/src/application/control.rs:180`：人的普通 resume 先把未知工具调用写为核查上下文，不直接再执行该工具。
- `rust/anchor-runner-host/src/application/recovery.rs:219`：恢复决定只针对当前 cursor/attempt 的 WaitingRecovery 接受。
- `rust/anchor-runner-host/src/application/recovery.rs:107`：取得 lease 后再次独立检查 cursor/attempt/WaitingRecovery，避免写入迟到决定。
- `rust/anchor-runner-host/src/application/recovery.rs:134`：Abort 结算；其余保留未解决 attempt，全部关闭后才续跑同一 Run。

仅合并后：

- `rust/anchor-runner-host/src/application/legacy_run_view.rs:31`：旧 WaitingRecovery 解释为 Effect wait，不伪装成普通人等待。

runtime（合并后，Host 数之外）：

- `rust/anchor-runtime/src/graph/runner.rs:304`：旧串行路径保留 cursor 后失败。
- `rust/anchor-runtime/src/graph/parallel.rs:155`：旧并行路径保留现场并阻断新派发。
- `rust/anchor-runtime/src/graph/scheduler.rs:119`：原生调度器用 Code+Observation 停给人处理。

Kernel（合并后，Host 数之外）：

- `rust/anchor-kernel/src/kernel.rs:418`：已有同 endpoint 未知效果时不自行再次执行；效果事实归 Kernel。

### 预算耗尽如何处理（3 → 4）

预算检测及状态写入属于 runtime；原生 BudgetExhausted 结果仍落 unsupported 分支，M2 才接 Code+Observation stop。Kernel 自身账本预算硬上限的 Escalated 与该执行端口结果不是同一层规则，W4 未决定。

两侧相同：

- `rust/anchor-runner-host/src/application/control.rs:249`：旧预算停机不可直接普通 resume。
- `rust/anchor-graph-host/src/graph_call.rs:342`：内联 wait child 白名单包含 BudgetStopped，决定是否重入。
- `rust/anchor-graph-host/src/graph_call.rs:629`：子 budget stop 投影为 Waiting，不向父报告完成。

仅合并后：

- `rust/anchor-runner-host/src/application/legacy_run_view.rs:21`：新读取把旧 BudgetStopped 显示为 Human wait，reason=budget。

runtime（合并后，Host 数之外）：

- `rust/anchor-runtime/src/graph/runner.rs:615`：旧串行执行结果变 BudgetStopped。
- `rust/anchor-runtime/src/graph/parallel.rs:242`：旧并行执行结果变 BudgetStopped。
- `rust/anchor-runtime/src/graph/scheduler.rs:211`：原生 BudgetExhausted 目前未实现（Unsupported）。

Kernel（合并后，Host 数之外）：

- `rust/anchor-kernel/src/kernel.rs:457`：Kernel 账本预算硬上限结算为 Escalated；与执行端口预算停止分层。

### 子 Run 结算后如何释放父 Run（3 → 3）

仍是旧 Graph call 与旧 parent cursor 机制；原生 scheduler 的 validate_probe 仍拒绝 Op.call，不能把本表当作 M2 Child wait 已实现。Session receipt 用同一 child_finished 唤醒，不重复计。

两侧相同：

- `rust/anchor-graph-host/src/graph_call.rs:577`：已完成子 Run 导出结果；等待/失败/未知分别转为调用观察，供父 Runner 决定推进。
- `rust/anchor-runner-host/src/application/dispatch.rs:149`：独立完成回调只在 child Completed、delivery 非 pending、wait 模式且父无 worker 时触发唤醒。
- `rust/anchor-runner-host/src/application/control.rs:74`：唤醒写入前再次要求父 WaitingCall 与同一 node/invocation，防止旧回调作用于新位置。

runtime（合并后，Host 数之外）：

- `rust/anchor-runtime/src/graph/runner.rs:432`：旧 Runner 收到 child Completed 后冻结 call result 并推进。
- `rust/anchor-runtime/src/graph/scheduler.rs:16`：原生 Op.call 当前 Unsupported。

## Session、授权与其他边界

以下保留位置不纳入上表的 Run 全局规则数。它们不是可以凭“换了字段”直接删掉的规则；特别是 Session-call 恢复写入与 Run 恢复编排，要在 M3 检验是否仍有同一事实的两个写入者。

| 位置（合并后） | 归属与用途 |
| --- | --- |
| `rust/anchor-runner-host/src/application/conversations.rs:872` | Run 结算到 Turn 结果/投递责任的映射归 Session，不是 Run 的第二份终态事实。 |
| `rust/anchor-runner-host/src/application/conversations.rs:386` | paused/budget 的 Turn 仍占用会话；物理 Run 静止不等于可建立后继 Turn。 |
| `rust/anchor-runner-host/src/application/conversations.rs:653` | 旧 Turn 被替代后不能恢复执行；Session 线性化规则。 |
| `rust/anchor-runner-host/src/application/assistants.rs:41` | current assistant 指针属于 Session；退役规则不是 Run 的 resume 定律。 |
| `rust/anchor-runner-host/src/application/run_projection.rs:71` | 身份、scope、assistant 当前性收窄 advisory actions；写入仍在用例校验。 |
| `rust/anchor-runner-host/src/application/control.rs:62` | Session call 不通过普通 pause/resume，拥有协调生命周期。 |
| `rust/anchor-runner-host/src/application/session_calls.rs:313` | Session call 的 continuation 门禁，不能当成普通 Run resume 的统一规则。 |
| `rust/anchor-runner-host/src/application/session_calls.rs:369` | Session call 自己回填未知调用的核查观察；仍有独立编排写入，M3 需要核对事实所有者。 |
| `rust/anchor-runner-host/src/node_host/assistant.rs:9` | Turn 被替代/停止后抑制旧回复；不属于 Run 晚到节点提交门禁。 |
| `rust/anchor-runner-host/src/api/wecom.rs:692` | 网关是否停止等待及投递错误由 Session/通道解释 Run 事实。 |
| `rust/anchor-runner-host/src/api/turns.rs:35` | Turn 流停止条件含 Stopped，因此不是 Run 终态谓词。 |
| `rust/anchor-runner-host/src/node_host/conversation.rs:125` | 历史上下文选择允许停下的 Run；不是 Run 结算判断。 |
| `rust/anchor-runner-host/src/api/files.rs:196` | 产物下载要求已完成或有冻结结果，属于产物可见性。 |
| `rust/anchor-runner-host/src/application/recovery.rs:14` | 原生 attempt 决定 API Unsupported；必须作为能力缺口保留。 |

## 宪法的三个条件

| 条件 | 本次答案与边界 |
| --- | --- |
| 新增/改动组件是否由宪法推出，或可整块替换 | Run phase/动作读取是事实投影；worker 是 Host 观察；原生 pause 文件是待消费命令，由边界转成提交，不是第二份 Run phase；Goose ACP/模型传输与 HTTP/Web 是边缘。`run_json`、`legacy_run_view` 与双存储归属桥仍是有期限的兼容边缘，M3 切换和删除旧实现、M4 清理剩余桥；当前尚未退役。 |
| 核心是否需要改动 | 本次无需改动 `anchor-kernel`。实际执行 `git diff 6ee2b0e --stat -- rust/anchor-kernel`，输出为空。M2 的并行、Child wait、人工恢复尚未实现，不能从本次静态盘点推断以后也无需改动。 |
| 核心中有没有可删的东西 | 本次没有发现可提出并验证的删除项。裁决/身份/账本成本、版本与等待门禁、只追加和结算仍有必要职责；本次未进行 Kernel 删除实验，不能把“零改动”写成已经证明最小。新增 Host 兼容桥不属于核心，按上述退役节点处理。 |

## 第 7 节四个极限

| 极限 | 本次可核对的结构事实 | 尚未证明或未实现 |
| --- | --- | --- |
| 智能无限 | 模型不拥有 phase、身份或成本写入；Host 确定性地盖章并提交，pause 也由 Host 代码转为控制提交。 | 这里只检查所有权，不是更强模型的实测；原生并行、Op.call、助手操作仍未接入。 |
| 规模无限 | Run 按所有者独立存储，worker 注册与控制按 Run 定位；读取观察不决定执行事实。 | `records()` 扫描、单进程注册、串行 probe 均不构成无限规模证明；M2 尚无原生并发/Child wait，未做压力或复杂度实测。 |
| 时间无限 | Kernel 结算不可逆、晚到提交不改已结算事实；Host 缺少终结观察保留未知，兼容桥有明确退役阶段。 | 本次未做多年历史/生产数据离线迁移；pause 意图清理与 Goose 原生会话长期兼容不在静态表的验证范围。旧 Runner 仍存在。 |
| 存在对手 | Kernel 的授权/等待/结算/版本门禁仍是事实入口；未知效果不通过读取投影伪装成成功，Agent 不获得 Host 控制身份。 | 本次没有新增对抗测试或真实 Provider 验收；沿用边界不能当作完整攻防证明。M2 attempt 恢复接口仍不支持原生路径。 |

## 行数辅助信号与未关闭项

相对 `1d1ba54`，本次盘点工作树中 `anchor-runner-host` 为 +5711/−763，`anchor-graph-host` 为 +982/−36，`anchor-runtime` 为 +415/−26（`git diff --numstat 1d1ba54 -- <crate>`，含测试）。明显新增来自 master 合入的启动/配置/Plugin 能力和其测试，以及 R/R2/M1 的新投影、pause 与回归；旧实现尚按计划保留，因此这不是“删除旧 Runner”的完成数据。行数不作为通过门槛。

未关闭项是原生并行、Op.call、助手 host 操作、BudgetExhausted/其余执行结果、attempt 人工决定、内核 Run 删除能力，以及随 M3 门面切换删除旧写入路径。关注规则迁到 Web、静止仍有 Web 副本、旧/原生 resume 写入前置条件未统一，均如实保留。真实 Goose/Provider、全量测试与合并验收结果由 [verification.md](verification.md) 记录；本表不把静态阅读计为端到端通过。
