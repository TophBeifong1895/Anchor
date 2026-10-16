# Constitution Host 接入前置探针

这是当前公开 API 的反例观察，结论为 **preflight blocked**，不是 H1–H6 的功能验收。它依赖真实 `anchor-kernel` 与 `GraphValidator`，只构造单节点 Op Graph；不运行执行器、Goose 或真实 provider，不修改内核及主 Rust workspace。

源码基线：`d0f9b61`（本次 Host 接入任务的起点）。

三个阻塞观察：

- 未知效果由真实 `Code` 身份提交 `stop`，被 `HumanOnly` 拒绝，Run 仍执行。当前 Graph 校验器也不接受其他请求，不能按计划通过内核生命周期进入等待人。
- 预算为 10，连续三次各报告成本 6 的非法内容均被打回，持久化 `spent` 仍为 0，未结算。
- 历史存在残缺尾行后，后续接受的 `stop` 状态已写入，但历史仍不可读。仅在拥有的临时 fixture 内还原旧 `resource.json`，模拟记录已持久化而状态写入丢失；重新构造 Kernel 后，接受的停止事实无法恢复，Run 回到版本 0 的执行态。

另观察停止后的迟到 Code 结果仍可被接受；调度器可丢弃取消执行者的迟到结果，因此不单独列为必须修改内核的阻塞。

`main` 成功退出只表示观察过程执行完成；输出明确标记 `preflight_blocked`。这些观测没有被写成主 workspace 中的绿色功能断言。文件损坏和状态写入丢失模拟全部限制在 `tempfile` 创建的独立目录，结束后自动回收。

从仓库根执行以下命令（`--offline` 要求本机已有锁定依赖缓存）：

```sh
cargo fmt --manifest-path experiments/constitution-host-preflight/Cargo.toml
cargo fmt --manifest-path experiments/constitution-host-preflight/Cargo.toml --check
cargo check --offline --locked --manifest-path experiments/constitution-host-preflight/Cargo.toml
cargo clippy --offline --locked --manifest-path experiments/constitution-host-preflight/Cargo.toml --all-targets -- -D warnings
cargo run --offline --locked --manifest-path experiments/constitution-host-preflight/Cargo.toml > experiments/constitution-host-preflight/evidence.json
```

以上 `fmt`、`fmt --check`、`check`、`clippy` 和 `run` 均已实际执行，退出码为 0。本目录的 `evidence.json` 保存实际执行所得 JSON。Cargo.lock 固定此独立探针的依赖；主 workspace 及内核均未修改。阻塞对应计划 C1 与第 5 节的停止条件，后续应审查内核通用语义后再推进 Host 迁移。
