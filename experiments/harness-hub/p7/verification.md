# P7 验证与证据

2026-10-21，工作区起点 `b05b66a`。本轮不提交或清理其他工作流。`anchor-hub/signals.rs` 的 detail 形态合并改动在开工时已经存在，本轮只使用和验证，不将其算作本轮实现。

## 模型试验

| 命令/动作 | 实际结果与证据 |
| --- | --- |
| `python3 experiments/harness-hub/p7/run_blind.py prepare --root .local/harness-hub-p7/blind-01`，随后 `run` | 模型前 MCP 入口路径被 Host 拦截，0.631 秒；[终态](../../../.local/harness-hub-p7/blind-01/response.json)，该目录无 ACP 模型证据 |
| 同一脚本 `prepare --root .local/harness-hub-p7/blind-02` | 沙箱内只读 MCP 初始化及 signals 成功，14 条；[preflight](../../../.local/harness-hub-p7/blind-02/mcp-preflight.json) |
| 同一脚本 `run --root .local/harness-hub-p7/blind-02` | 唯一一次模型试验，9 个 Agent invocation、319 个模型请求；评审 REVISE；gate exit 127；[meta](../../../.local/harness-hub-p7/blind-02/meta.json)、[response](../../../.local/harness-hub-p7/blind-02/response.json) |
| `python3 experiments/harness-hub/p7/collect.py .local/harness-hub-p7/blind-02 .local/harness-hub-p7/collected` | 成功，19,949,171 tokens、12 次 records、凭据字节命中 0；归档前扫描，通过后生成原始压缩包 |

Host 从 `1af26b1` 构建，二进制身份见 [identity](../../../.local/harness-hub-p7/blind-02/identity.json)，[构建日志](../../../.local/harness-hub-p7/build/host.log) 保留。当前部署只用于取得平台默认模型配置和 Goose 环境，没有重启、部署或修改当前服务。私有模型配置在隔离执行结束后移除。

## 确定性与静态检查

| 实际执行的命令 | 结果/原始输出 |
| --- | --- |
| `cargo build --manifest-path rust/Cargo.toml -p anchor-hub --locked` | 通过；终端输出未另存文件 |
| `cargo test --manifest-path rust/Cargo.toml -p anchor-hub --locked` | 17 项功能 + 3 个围栏通过；本会话终端已执行，原始输出未另存文件，不能以本报告代替原始日志 |
| `cargo test --manifest-path rust/Cargo.toml -p anchor-runtime --locked` | 155 lib + 7 integration + 2 doc，通过 164、失败 0、忽略 0；[日志](../../../.local/harness-hub-p7/build/runtime-tests.log) |
| `cargo clippy --manifest-path rust/Cargo.toml -p anchor-runtime -p anchor-hub --all-targets --locked -- -D warnings` | 通过；[日志](../../../.local/harness-hub-p7/build/clippy.log) |
| `cargo fmt --manifest-path rust/Cargo.toml --all -- --check` | exit 0，无格式输出；[日志](../../../.local/harness-hub-p7/build/fmt.log) |
| `python3 -m unittest discover -s experiments/harness-hub/p7 -p test_graph.py -v` | 原 gate 6 项通过；解释器修正后 7 项通过，新增真实 bubblewrap 中不挂载 `/etc/alternatives` 的 PASS 场景；[最终日志](../../../.local/harness-hub-p7/build/gate-tests.log) |
| `cargo test --manifest-path rust/Cargo.toml -p anchor-runtime --locked shipped_graph_examples_compile_without_runtime_or_external_services` | 解释器修正后，包含新 Graph 的实际测试 1 项通过；[日志](../../../.local/harness-hub-p7/build/graph-final-correct-filter.log)。曾误用 `shipped_example_graphs_compile_and_admit` 过滤，0 项，未计为通过证据，原输出留在 graph-final.log |
| `python3 -m py_compile experiments/harness-hub/p7/run_blind.py experiments/harness-hub/p7/collect.py experiments/harness-hub/p7/replay_gate.py experiments/harness-hub/p7/test_graph.py` | 通过 |
| `bash scripts/check-sensitive-wording.sh --all` | 最终工程文本检查，结果见 [日志](../../../.local/harness-hub-p7/build/wording-final.log)；先前误用 Python 启动 shell 脚本导致语法错误，已用正确入口执行 |
| `git diff --check` | 通过；只检查已跟踪 diff，新文件另作 JSON/语法/措辞检查 |
| `git diff --exit-code 9a5fa57 -- rust/anchor-kernel` | exit 0，Kernel 零 diff |
| `git diff --exit-code -- examples/graphs/rsi.json` | exit 0，旧 RSI 未改 |

全量 Runtime 和 Clippy 在切片完成时执行一次；后续仅 gate 的 shell 启动与实验文件/文档变化，使用 gate 定向检查、Graph 作者编译和确定性回放收尾。未改变 Web，不运行 Web 检查；未重跑整个 Rust workspace 或大型业务 Graph。

## gate 无模型回放

`python3 experiments/harness-hub/p7/replay_gate.py .local/harness-hub-p7/collected .local/harness-hub-p7/gate-replay-03`

Op-only fixture 使用保存的 proposals.json、synthesis.md、review.md 和同一旧 Host；gate 使用修正后的解释器选择。**gate exit 0、route=triage，模型请求 0**。triage 被明确的退出节点替换，所以整个 fixture 终态 failed 是预期停止，不能写成整图成功或模型修订完成。证据：[gate manifest 与产物](../../../.local/harness-hub-p7/gate-replay-03/state/artifacts/)、[核对结果](../../../.local/harness-hub-p7/gate-replay-03/replay-result.json)。

此前两次 Op-only fixture 装配分别缺 manifest.plugins、缺 publish 上游声明，均在 Graph 接纳时失败，无模型请求；原目录 gate-replay、gate-replay-02 保留，不混入盲测成本。原盲测 Graph 已冻结为 [trial-graph.json](trial-graph.json)，修正后没有真实模型重跑。

## 证据完整性与限制

[score.json](score.json) 记录身份、原始包和工具流的 SHA-256、全部 Artifact manifest 哈希、判分与未通过门槛。[metrics.json](metrics.json) 移除逐请求 samples 以便审阅，完整 samples 留在本地原始 metrics 与 ACP。proposals.json 和 trial-graph.json 与冻结原件逐字节一致。

轨迹包是本机可获取的真实原始产物，包含每个节点的 Goose 原生会话数据库及 WAL、ACP、Kernel 历史、所有已提交 Artifact 和 bundle。未把凭据配置装入包；未嵌入全部 A 数据或源码副本，它们由 source-manifest 的逐文件哈希定位。原始包与全部工具观察在 `.local/harness-hub-p7/collected/`，不入库。

内容判分基于最终综合稿及分支原文；不是新一轮模型评判。没有实施工具修复、提议账本、下一轮数据判定、旧 RSI 同输入基线、修订循环或完整发布的真实 Provider 验收。
