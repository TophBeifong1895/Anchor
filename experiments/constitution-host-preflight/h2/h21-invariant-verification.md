# H2.1：S18 不变量通过，旧覆盖率门槛未通过

日期：2026-10-17。源码起点：21a3c91，任务依据为本轮重写的 docs/constitution-h2-task.md。提交标题：wip(validator): verify S18 invariants and retain coverage gate，以标题定位包含本文件的提交。

**H2.1 尚未全绿，P 探针未开始。** 两个独立用例及全部 5,000 个并行 seed 的不变量检查通过，组末原有 parallel_activations >5000 断言失败，实际 4775。这次没有新的语义例外；S18 已批准，未改内核或 H2.1 之外的生产裁决。

## 对照实现与完整结果

统一检查失败结局，不再逐项区分“单失败”“同波多失败”“上限先结算”：

1. 原因正当：唯一终结记录的 detail 必须来自实际失败，或来自已达上限的分支节点。后者要求旧侧已记录该上限，或该节点是已完成分支调用的后继，且 passes 达到 limit。
2. 事实不丢不重：忽略 StaleVersion 重提后，每个实际失败恰有一条 Code Settle，匹配槽位和 reason；每个旧侧 Work 结果恰有一条 Agent Write，匹配调用、commit 及 Evidence::Resource。接受者在状态中，迟到者只在 TaskSettled 历史中；拒绝集合与状态集合不相交。不存在额外失败或 Work 提交。
3. 其余相等：仅在测试比较副本中还原迟到事实，扣除经过验证的末次上限 ceased / refuse_outgoing 效果，再逐项比较 Graph 事实、完整 Work 结果与完整分支。被上限覆盖的边决定从此前接受的状态恢复，不清空其他边。沿用 S6 对旧侧未执行 cursor 的归一化；真实失败调用即使没有结果仍保留其计数。

删除被统一规则替代的 first_failure checkpoint、旧按情形 helper 和旧无结果计数 helper；不删除测试。seed 495 / seed 2122 独立用例按批准的新规则验收，分别覆盖多失败与上限抢先结算。

全部 seed 都已执行完，没有中途语义断言失败；并行测试在最后原有覆盖门槛失败。三个新计数门槛均已满足，原概率约 3%、哈希、seed 范围、Tally 定义、原数值门槛均保持不变。

| Tally | serial（10000 个） | parallel（5000 个） | calls（5000 个） |
| --- | ---: | ---: | ---: |
| cases | 10000 | 5000 | 5000 |
| rejected_graphs | 0 | 0 | 0 |
| completed | 6968 | 3087 | 2902 |
| round_limited | 2260 | 808 | 1218 |
| dispatches | 26981 | 33107 | 9733 |
| longest | 21 | 20 | 13 |
| module_activated | 1310 | 0 | 721 |
| module_limited | 293 | 0 | 156 |
| skipped_paths | 9046 | 1865 | 4576 |
| failed（分支上限） | 0 | 330 | 0 |
| node_failed | 772 | 775 | 0 |
| multi_failure_waves | 0 | 6 | 0 |
| ceiling_preempted_failures | 0 | 2 | 0 |
| parallel_activations | 0 | **4775** | 0 |
| resubmissions | 0 | 10224 | 0 |
| phantom_invocations | 0 | 182 | 0 |
| late_refusals | 0 | 357 | 0 |
| calls_waited | 0 | 0 | 1704 |
| calls_detached | 0 | 0 | 1485 |
| call_failed | 0 | 0 | 497 |
| call_unknown | 0 | 0 | 383 |

multi_failure_waves=6，小于 10，未调注入概率凑数。

## 当前停止条件与审查问题

rust/anchor-runtime/src/graph/validator/differential.rs:1251 仍从已接受的 fanout 结果计数 parallel_activations。该指标实际为 4775；第 1295 行保留原断言 >5000。其他覆盖项（resubmissions >1000、phantom_invocations >0、late_refusals >0、completed >100、failed >100，以及新增失败计数）均满足。

节点失败注入使部分 Run 在后续并行激活前终结，继承自无失败对照的激活门槛没有通过。没有将状态和历史重复计数，也没有改统计单位或增加 seed 以绕过任务书约束。任务书 §3 禁止下调旧阈值，§4 要求在需要弱化断言 / 下调阈值时停止。因此保留该失败，提交可编译 WIP，P 尚未实施。

建议审查决定：是否将原无失败覆盖回归与新增失败注入回归分为两组，原组保留原门槛，新增组继续固定当前失败概率、seed 范围与三个失败计数门槛。该方案尚未实现或验证；不会把这次不满足门槛的结果标为通过。也未自行把 >5000 改成其他数值。

## 实际验证

从 rust 目录执行。首次基线在受限沙箱运行；Runtime 与任务书预期相同（125 通过 / 2 失败），但 Host 因无法创建 Bubblewrap 网络命名空间、无法监听本地端口等环境权限原因多处失败。已将这一差异报告并在沙箱外执行四 crate 完整回归。外部回归使用本轮改动后的源码，不冒充改动前的无沙箱基线。

| 命令 | 实际结果 |
| --- | --- |
| cargo build -p anchor-sandbox-bwrap --bins | 通过；原始输出仅本轮工具会话，未另存 |
| cargo test -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --no-fail-fast（首次，沙箱内） | Kernel 37 通过；Runtime 125 通过 / 2 失败，另 2 doctest 通过；graph-host 46 通过（含 4 项 child_lifecycle）；runner-host 758 通过 / 316 失败 / 112 忽略。环境基线不符，已报告并复核；退出码 101 |
| cargo test -p anchor-runtime --lib graph::validator::differential -- --nocapture（初次定向） | 4 通过 / 1 失败，全部 seed 走完，旧激活门槛失败；退出码 101 |
| cargo fmt --all -- --check | 通过 |
| cargo clippy -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --all-targets -- -D warnings | 通过 |
| cargo test -p anchor-runtime -- --nocapture differential（最终定向） | 4 通过 / 1 失败；串行、调用、两个独立用例通过，并行仅组末旧覆盖门槛失败；退出码 101 |
| cargo test -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --no-fail-fast（沙箱外，本轮改动后） | Kernel 37 通过；Runtime 126 通过 / 1 失败，另 2 doctest 通过；runner-host 1074 通过 / 0 失败 / 112 忽略；graph-host 46 通过 / 0 失败。Host 合计 1120 通过 / 112 忽略，与预期合计一致。唯一失败为并行覆盖门槛，退出码 101 |
| git diff 6ee2b0e --stat -- rust/anchor-kernel | 空 |
| git diff --check | 通过 |

原始输出见 [首次受限基线](h21-invariant-sandbox-baseline.log)、[最终差分与全部 Tally](h21-invariant-final.log)、[Clippy](h21-invariant-clippy.log)、[沙箱外完整回归](h21-invariant-workspace.log)。日志仅去掉行尾空白及文件末尾多余空行，未删改内容。没有把首次沙箱失败当作新增功能问题。

## 结构及未覆盖边界

- C1：内核相对 6ee2b0e 无改动。
- C3：只改 cfg(test) 下的 differential 模块，生产 validator 无新增 I/O、async 或端口调用。
- C4：scheduler.rs 未创建，P 生命周期路径未实施，无新增轮询。
- C5：沿用已实现的 Code-only Work Settle，本轮验证历史身份；成本盖章接线尚未实施。
- 新的语义差异：无。S18 已批准，完整 Tally 如上，H2.1 尚未全绿。
- P 测试 1–7：均未执行，先行 H2.1 仍有原门槛冲突；measurement.md 尚未创建。真实 Goose：未执行，尚未开始 Host 探针。真实 Provider、Web、部署均未执行。
- 原 H2.2 / H2.3 已由新版任务书取消，后续是探针 P，未恢复旧任务范围。
