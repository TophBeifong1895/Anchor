# H2.1 WIP：并行多失败停止交回

日期：2026-10-17。源码起点：469d81f，加任务书指明的未提交 Settle 半成品。**H2.1 未验收，H2.2 / H2.3 未开始**。本次是本地确定性对照，不包含真实 Provider、固定 Goose、Web 或部署验收。

## 停止现场

任务书 §5 第 3、4 条要求：旧 Runner 与 §1 语义不一致，或对照出现无法用已登记语义解释的差异时立即停止。已停止功能推进，保留失败断言并提交可编译 WIP。

随机并行 Graph seed 495，同一波的 b1_0#1 与 b2_0#1 均命中确定性失败：

- 旧 record.error：b2_0 failed: fixture failure b2_0 #1。
- 新 Phase::Settled 的 detail：b1_0 failed: fixture failure b1_0 #1。
- 旧执行器 rust/anchor-runtime/src/graph/parallel.rs:271 的失败分支在每次处理失败结果时覆盖 record.error（第 278 行）；第 284–290 行收束时已有 Failed 状态，故不会恢复最初错误。
- 新校验器首次 Settle 已结算；后续提交由未修改的内核 TaskSettled 门禁拒绝，不能覆盖 detail。

S6 原记录的是并行分支轮次上限触发后的迟到成功结果，不包含多次已知节点失败的错误文本选取。不能自行放宽“旧 record.error 与新 detail 相等”的要求，也不能以调整哈希避开多失败来通过门槛。

待审查决定：是否接受“首次被内核接受的节点失败决定唯一结算原因；后续失败只保留在提交历史中”，并明确对照错误文本的映射规则。S17 仅登记 WIP 差异，未擅自批准这项变化。

## 实现范围

- SettleSubmission 只含 reason，拒绝多余字段；仅 Code、开放 Work 槽可提交。
- count_dispatch 保留开始计数；不写结果与出边决定；并行分支 Failed/error 保留，已完成兄弟结果保留。
- 新增 4 个 settle_tests；串行/并行随机差分加入约 3% 哈希失败注入，调用组保持原脚本。
- 原阈值不下降。原 failed 继续只计并行轮次上限失败，node_failed 单独计数。原 ceased 断言仅适用于轮次上限，任务书明确要求将该分支与节点失败分开。
- 没有内核、旧 Runner、Host 或 H2.1 以外校验裁决修改。

## 实际验证

均在 rust 目录运行；原始输出见下方链接。入口第一次 build 因 judge_settle 缺失失败，所以 test 基线未运行；参考 6ee2b0e 交接的 1278 通过 / 112 忽略，不冒充本轮基线。

| 命令 | 实际结果 |
| --- | --- |
| cargo build -p anchor-sandbox-bwrap --bins | 修复半成品后通过 |
| cargo fmt --all -- --check | 通过 |
| cargo clippy -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --all-targets -- -D warnings | 通过 |
| cargo test -p anchor-kernel | 37 通过，0 失败，0 忽略（12 + 17 + 8） |
| cargo test -p anchor-runtime --lib graph::validator -- --nocapture | 12 通过，1 失败，0 忽略；新增 4 项通过 |
| cargo test -p anchor-runtime -- --nocapture differential | 串行、调用通过；并行 seed 495 失败，退出码 101 |
| cargo test -p anchor-runtime | 124 通过，1 失败，0 忽略；退出码 101，未继续运行 doctest |
| cargo test -p anchor-runtime --doc | 独立执行，2 通过 |
| cargo test -p anchor-runner-host -p anchor-graph-host | 1120 通过，0 失败，112 忽略；退出码 0 |
| cargo test -p anchor-runtime parallel_regions_match_the_runner_on_random_graphs -- --nocapture | 最终 Tally 口径修正后仍在 seed 495 失败 |
| git diff 6ee2b0e --stat -- rust/anchor-kernel | 空 |
| git diff --check | 通过 |

本轮 Runtime 新增 4 项测试，无删除、新增 ignore 或降低阈值；另有 1 个原差分测试暴露新失败注入下的语义冲突。Host 代码、测试及 ignore 均未改动。

- [首次定向输出](h21-targeted.log)、[完整 Runtime 输出](h21-runtime.log)、[差分与完整 Tally](h21-differential.log)、[最终并行复现](h21-parallel-final.log)、[Clippy 输出](h21-clippy.log)、[doctest 输出](h21-doc.log)。
- Host 完整原始输出仅本机 /tmp/anchor-h21-host.log；已检查所有目标汇总：1120 / 0 / 112，未把该临时路径当作长期证据。Kernel 和 build 的原始输出只在本次工具会话中，未另存文件。

## 差分 Tally

串行：cases 10000，completed 6968，round_limited 2260，dispatches 26981，longest 21，module_activated 1310，module_limited 293，skipped_paths 9046，node_failed **772**；其余计数 0。

调用：cases 5000，completed 2902，round_limited 1218，dispatches 9733，longest 13，module_activated 721，module_limited 156，skipped_paths 4576，calls_waited 1704，calls_detached 1485，call_failed 497，call_unknown 383，node_failed **0**；其余计数 0。

并行：未完成，在 seed 495 的错误文本断言失败，**没有全组 Tally，node_failed 全组数未知，不能声称 >100**。保留原有规模、断言与阈值，待语义决定后继续验收。

## 结构与交回

| 项 | 结果 |
| --- | --- |
| C1 | git diff 6ee2b0e -- rust/anchor-kernel 为空 |
| C3 | 生产 validator.rs 无 I/O、async、端口调用；新增 settle_tests 为同步测试。原有 differential 的异步 fixture 仍只在 cfg(test) 下 |
| C4 | scheduler.rs 未创建，本刀不适用；未新增停止轮询 |
| C5 | Settle 的非 Code 身份被拒；调度器及成本盖章尚未实施 |

刀：H2.1 WIP；提交标题为 wip(validator): preserve H2.1 parallel failure mismatch for review，以该标题定位本证据所在提交。改动文件见该提交；任务书最初为未跟踪文件，随状态结论一起纳入。

新增语义条目：S17（未验收），首次节点失败结算与旧侧同波覆盖错误的差异。

未执行：H2.2 的 2000 + 2000 seed Host 对照、FileStore 重启、并发/version、stop 测试，以及 H2.3 wiring-survey；原因均为先行 H2.1 已触发停止条件。固定 Goose 忽略项未显式执行，真实 Provider 未执行。没有宣称 H2 或新 Host 路径完成。
