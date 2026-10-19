# M2 并行 Call 准入边界

任务书 §10.12 已裁定不扩展校验器。Host 自 master `82a726d` 起，在共用 `reject_snapshot` 中具名拒绝 fanout/join 区域内的 `Op.call`。M2 的 Kernel 创建也复用该入口；这是既有产品准入规则，不是已准入 Graph 在 Kernel 路径上的 Unsupported。

HTTP 回归 `parallel_call_is_rejected_by_shared_admission_without_persisting_parent_or_child` 对默认与 `execution=kernel_probe` 两条路径提交同一 Graph，均得到 422 和 `Op.call node \`nested-call\` is unsupported inside a fanout/join region`，且没有 Run、子 Run、metadata 或 Artifact 副作用。原始输出：`/tmp/anchor-m2/section-10-12/parallel-call-admission.log`（1 / 0 / 0）。

[原诊断](parallel-call-diagnostic.rs) 只直接调用 GraphRunRecord/GraphValidator，绕过 Host 产品准入，证明了底层 frontier 的行为；它不能证明产品允许该组合。先前“现有准入允许”的结论有误，建议的校验器扩展已退出本轮范围。串行 Call、并行 Work 与模块继续由现有差分覆盖。
