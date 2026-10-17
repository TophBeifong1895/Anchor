# H2.1 收尾验收

2026-10-17。起点 8a7ee01，按用户“继续”指令采用上次交回的分组建议：原无失败并行回归保留原覆盖率门槛；新增失败组仍以原哈希、约 3% 概率、同一组 5000 个 seed 检查 S18。没有下调旧门槛、改变失败组概率、扩展 seed 范围或重复计数。

S18 原因正当、事实不丢不重、其余相等三项不变量覆盖全部失败组；seed 495 和 2122 独立用例通过。旧无失败组 parallel_activations=5048，满足原 >5000；失败组对应数值 4775 如实报告，不冒充覆盖门槛结果。

实际命令及结果：

- cargo test -p anchor-runtime -- --nocapture differential：5 通过 / 0 失败，三个随机组及两个独立用例均通过。
- cargo test -p anchor-runtime：127 通过 / 0 失败 / 0 忽略，另 2 doctest 通过。
- cargo fmt --all -- --check：通过。
- cargo clippy -p anchor-runtime --all-targets -- -D warnings：通过。
- Kernel 与 Host 此后未改，复用上轮完整回归的 Kernel 37、Host 合计1120通过 /112忽略；不重复运行。
- git diff 6ee2b0e -- rust/anchor-kernel：空。

原始输出：[差分与全部 Tally](h21-complete-differential.log)、[Runtime](h21-complete-runtime.log)、[Clippy](h21-complete-clippy.log)。上轮 [覆盖门槛现场](h21-invariant-verification.md) 保留为历史。

## 完整 Tally

```text
differential calls: Tally { cases: 5000, rejected_graphs: 0, completed: 2902, round_limited: 1218, dispatches: 9733, longest: 13, module_activated: 721, module_limited: 156, skipped_paths: 4576, failed: 0, node_failed: 0, multi_failure_waves: 0, ceiling_preempted_failures: 0, parallel_activations: 0, resubmissions: 0, phantom_invocations: 0, late_refusals: 0, calls_waited: 1704, calls_detached: 1485, call_failed: 497, call_unknown: 383 }
differential: Tally { cases: 10000, rejected_graphs: 0, completed: 6968, round_limited: 2260, dispatches: 26981, longest: 21, module_activated: 1310, module_limited: 293, skipped_paths: 9046, failed: 0, node_failed: 772, multi_failure_waves: 0, ceiling_preempted_failures: 0, parallel_activations: 0, resubmissions: 0, phantom_invocations: 0, late_refusals: 0, calls_waited: 0, calls_detached: 0, call_failed: 0, call_unknown: 0 }
differential parallel: Tally { cases: 5000, rejected_graphs: 0, completed: 3087, round_limited: 808, dispatches: 33107, longest: 20, module_activated: 0, module_limited: 0, skipped_paths: 1865, failed: 330, node_failed: 775, multi_failure_waves: 6, ceiling_preempted_failures: 2, parallel_activations: 4775, resubmissions: 10224, phantom_invocations: 182, late_refusals: 357, calls_waited: 0, calls_detached: 0, call_failed: 0, call_unknown: 0 }
differential parallel original coverage: Tally { cases: 5000, rejected_graphs: 0, completed: 3633, round_limited: 971, dispatches: 35958, longest: 20, module_activated: 0, module_limited: 0, skipped_paths: 1299, failed: 396, node_failed: 0, multi_failure_waves: 0, ceiling_preempted_failures: 0, parallel_activations: 5048, resubmissions: 11444, phantom_invocations: 220, late_refusals: 68, calls_waited: 0, calls_detached: 0, call_failed: 0, call_unknown: 0 }
```

并行 node_failed=775、multi_failure_waves=6（小于10，未调概率）、ceiling_preempted_failures=2；串行 node_failed=772；调用为0。H2.1 完成的是确定性本地对照；真实 Provider、Goose、Web与部署未执行。后续为新版任务书中的串行探针 P。
