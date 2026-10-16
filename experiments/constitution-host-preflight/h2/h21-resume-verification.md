# H2.1 继续实施交回：S17 通过，S18 待审查

日期：2026-10-17。起点：3d5d091。提交标题：wip(validator): record H2.1 ceiling and node failure conflict（以标题定位包含本证据的提交）。

**H2.1 尚未完成验收；H2.2 / H2.3 未执行。** 本轮仅完成已批准 S17 的差分对照，随后遇到任务书 §5 第 3、4 条停止条件。编译通过，保留失败测试，不自行扩大 S6/S17 或修改既有生产裁决。固定 Goose、真实 Provider、Web、部署均未新增验收。

## 已完成的 S17 对照

- 从旧侧实际派发及原失败哈希重算所有节点失败，不从最终 error 反推；保存首次失败现场，以 activation、分支 cursor 和 invocation 证明多失败来自同一波。
- 检查唯一接受的 Code Settle，其 slot、reason 与实际失败一致；其余失败逐条对应 Code Settle / Refused(TaskSettled) 历史，并验证旧侧各失败分支 Failed、新侧仅接受者 Failed。
- 区分 S6 迟到成功与 S17 迟到失败；按历史核对计数、边事实、完整并行分支及 Work 结果，不把失败调用当作无结果的幽灵计数删除。
- 新增 multi_failure_waves > 0；失败哈希、约 3% 概率、seed 范围和既有阈值不变。seed 495 独立用例通过，node_failed=1、multi_failure_waves=1。

本轮唯一 Rust 改动在 rust/anchor-runtime/src/graph/validator/differential.rs。judge_settle、生产校验器、旧 Runner、Host、内核均未修改。S17 已批准行的语义描述保持审查方版本。

## 新停止现场：seed 2122

实际仅一个节点失败：b1_0#2，reason 为 fixture failure b1_0 #2。

| 事实 | 旧 Runner | 新 Kernel + GraphValidator |
| --- | --- | --- |
| 最终失败文本 | b1_0 failed: fixture failure b1_0 #2 | parallel branch `b0_1` exceeded node round ceiling `1` |
| ceased | 空集合 | {b0_1@1} |
| b1_0#2 失败提交 | 本波处理为节点失败 | Code Settle，内容 {"reason":"fixture failure b1_0 #2"}，Refused(TaskSettled) |

新侧先接受成功的 b0_0#2；conclude 的 look-ahead 看到该分支下一节点 b0_1 已达到 max_rounds=1，立即结算。因此同波唯一节点失败 b1_0#2 到达时已经关闭。旧侧先收完本波，节点失败使 Run 终止，不再进入下一波轮次上限检查。

对应源码：

- rust/anchor-runtime/src/graph/validator.rs:367，parallel_frontier；382–390 行检查上限、写 ceased 并返回 Failed。
- rust/anchor-runtime/src/graph/validator.rs:693，conclude；715–723 行将 look-ahead 的 Failed 固化为结算。
- rust/anchor-runtime/src/graph/parallel.rs:271，旧侧逐项收集失败；284–290 行在进入下一波前返回。
- rust/anchor-runtime/src/graph/validator/differential.rs:1340，seed 495 S17 独立验收。
- rust/anchor-runtime/src/graph/validator/differential.rs:1350，seed 2122 独立复现；1396 行保留单失败错误文本完全相等断言。

任务书 §1 测试第 3 条明确要求只有一个失败时旧 error 与新 detail 完全相等。本例不是 S17 的同波多节点失败例外；原 S6 也没有明确批准该单失败文本映射。登记 S18 为待审查，未批准。原并行随机组和新增复现均保留失败。

需要审查方决定：是否接受“兄弟成功提交触发的轮次上限先结算时，保留该结算原因，唯一迟到节点失败只进入被拒历史”，并明确单失败对照中 error、ceased、计数、分支状态及迟到事实的映射；若不接受，需要另行授权修改 H2.1 之外的 conclude / frontier 裁决。不能仅把节点失败文本相等断言删掉。

## 实际验证

命令均从 rust 目录执行，git 命令除外。恢复基线与任务书一致，故继续实施 S17。

| 命令 | 实际结果 |
| --- | --- |
| cargo build -p anchor-sandbox-bwrap --bins | 基线及本轮改动后均通过；输出仅工具会话，未另存原始文件 |
| cargo test -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --no-fail-fast | 基线：Kernel 37 通过；Runtime 124 通过 / 1 失败（seed 495），另 2 doctest 通过；runner-host 1078 通过 / 112 忽略；graph-host 42 通过。退出码 101 |
| cargo fmt --all -- --check | 通过，最终文档编辑后亦复核通过 |
| cargo clippy -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --all-targets -- -D warnings | 通过 |
| cargo test -p anchor-kernel | 37 通过 / 0 失败 / 0 忽略 |
| cargo test -p anchor-runtime same_wave_node_failures_keep_first_reason_and_record_late_failure -- --nocapture | 1 通过，seed 495 |
| cargo test -p anchor-runtime single_node_failure_preserves_error_when_sibling_reaches_ceiling -- --nocapture | 1 失败，seed 2122，退出码 101 |
| cargo test -p anchor-runtime -- --nocapture differential | 3 通过 / 2 失败；串行、调用、seed 495 通过；并行随机组及 seed 2122 失败，退出码 101 |
| cargo test -p anchor-runtime | 125 通过 / 2 失败 / 0 忽略，退出码 101；失败后未运行 doctest |
| cargo test -p anchor-runtime --lib graph::validator -- --nocapture | 最后定向复核：13 通过 / 2 失败 / 0 忽略，退出码 101；含最新 completed-prefix Tally 诊断 |
| cargo test -p anchor-runner-host -p anchor-graph-host | 本轮未单独重复；上方恢复基线已跑两 crate，合计 1120 通过 / 0 失败 / 112 忽略，此后 Host 与生产代码均未改 |
| git diff 6ee2b0e --stat -- rust/anchor-kernel | 空 |
| git diff --check | 通过 |

与基线相比新增 2 个测试：seed 495 通过、seed 2122 失败；原随机并行差分的阻塞从 seed 495 移至 seed 2122。没有删除测试、新增 ignore 或下调阈值。最后仅增强失败诊断文字及前缀 Tally 输出后复跑定向组，未再重复完整 Runtime；完整 Runtime 与最终定向输出都保留。

原始输出（仅去除文件末尾多余空行，命令输出内容未删改）：[恢复基线](h21-resume-baseline.log)、[Kernel](h21-resume-kernel.log)、[S17 独立验收](h21-resume-s17.log)、[seed 2122 独立复现](h21-resume-ceiling.log)、[差分组](h21-resume-differential.log)、[完整 Runtime](h21-resume-runtime.log)、[最终定向复核](h21-resume-targeted.log)、[最终 Clippy](h21-resume-clippy.log)。此前 WIP 证据保留在 [verification.md](verification.md)。

## 差分 Tally

串行 10000 个、调用 5000 个全部完成。并行在 seed 2122 失败，以下仅为 seeds 0..2121 的已完成前缀，不包含失败 seed，不代表全组 5000 个验收结果。

| 字段 | serial（完整） | parallel（已完成前缀） | calls（完整） |
| --- | ---: | ---: | ---: |
| cases | 10000 | 2122 | 5000 |
| rejected_graphs | 0 | 0 | 0 |
| completed | 6968 | 1324 | 2902 |
| round_limited | 2260 | 363 | 1218 |
| dispatches | 26981 | 14096 | 9733 |
| longest | 21 | 18 | 13 |
| module_activated | 1310 | 0 | 721 |
| module_limited | 293 | 0 | 156 |
| skipped_paths | 9046 | 776 | 4576 |
| failed（分支轮次上限） | 0 | 135 | 0 |
| node_failed | 772 | 300 | 0 |
| multi_failure_waves | 0 | 1 | 0 |
| parallel_activations | 0 | 2015 | 0 |
| resubmissions | 0 | 4356 | 0 |
| phantom_invocations | 0 | 70 | 0 |
| late_refusals | 0 | 134 | 0 |
| calls_waited | 0 | 0 | 1704 |
| calls_detached | 0 | 0 | 1485 |
| call_failed | 0 | 0 | 497 |
| call_unknown | 0 | 0 | 383 |

multi_failure_waves 实际为 1（小于 10），未调整概率凑数。串行 node_failed > 100 已验收；并行虽然前缀已记录 300 次节点失败，完整随机组未通过，不能声称全组阈值已验收。

## 结构及未执行项

- C1：内核相对 6ee2b0e 的 diff 为空。
- C3：生产 validator 未改；新增异步 fixture / 测试只在既有 cfg(test) 差分模块内，没有新增生产 I/O、async 或端口调用。
- C4：scheduler.rs 未创建，本刀不适用。
- C5：沿用已实现的 Code-only Work Settle；本轮核对历史身份，未新增执行者或成本来源，调度器成本盖章未实施。
- 新语义条目：S18 待审查，轮次上限结算与同波唯一节点失败的先后差异；S17 已批准描述未改。
- H2.2 的 2000 + 2000 seed Host 对照、FileStore 重启、并发 / version、stop 测试：未执行，因 H2.1 停止条件尚未解除。
- H2.3 wiring-survey：未执行，先行 H2.1 未验收，不接线。Host 仍使用旧 Runner / RunStore。
- 固定 Goose 忽略测试、真实 Provider、Web、部署：未执行；本次确定性差分和 fixture 证据不替代这些验收。
