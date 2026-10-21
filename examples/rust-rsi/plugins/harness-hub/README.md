# Harness Hub Plugin

只读 stdio MCP，供 [rsi-signals Graph](../../../graphs/rsi-signals.json) 查询运行信号和证据。清单的 `bin/anchor-hub` 是构建产物，安装前必须补齐；仓库不提交二进制。

在仓库根构建，然后将清单与二进制组成候选目录：

```sh
cargo build --manifest-path rust/Cargo.toml -p anchor-hub --release --locked
mkdir -p /tmp/anchor-harness-hub-plugin/bin
cp examples/rust-rsi/plugins/harness-hub/plugin.json /tmp/anchor-harness-hub-plugin/plugin.json
install -m 755 rust/target/release/anchor-hub /tmp/anchor-harness-hub-plugin/bin/anchor-hub
```

通过既有 Plugin 安装入口将该目录安装为 `harness-hub`。Host 的命令授权须含 `anchor-hub`；不要把 `/local-inputs` 文件直接当作进程入口，沙箱要求入口是系统命令或已挂载的 Plugin/tool 路径。

gate 需要沙箱内可运行的 Python 3，依次探测 `python3`、`python3.12`；授权相应命令，并验证其链接目标在沙箱内可用。仅在宿主执行 `python3 --version` 不足以证明沙箱可启动它。

数据来源由操作员在 `ANCHOR_RUNNER_LOCAL_INPUTS_ROOT/rsi-signals/local-inputs.json` 配置；共享 Graph 与 Plugin 只引用沙箱内的 `/local-inputs/harness-hub-data`。为 `signals`、`investigate-1` 到 `investigate-4` 和 `explore` 各授予实际数据目录，其他节点不需要原始运行目录。例如一个节点：

```json
{
  "signals": {"harness-hub-data": "/srv/anchor-history"}
}
```

调查节点额外授予 `host-source`（工具代码的只读副本）；`explore` 额外授予 `anchor-rsi-source`（`anchor-rsi` 代码证据实现），以及同一份 `host-source`。这些授权只影响本节点，不能从 Graph 输入扩大。完整本机配置由 [P7 隔离运行器](../../../../experiments/harness-hub/p7/run_blind.py) 的 prepare 步骤生成。

Graph 的运行查询设计为使用中枢；当前数据目录也挂载给节点，shell 仍能绕过 MCP 直接读取，P7 explore 实际发生过。源码比较使用只读本地输入，不依赖旧 `rsi` 示例中没有连接地址的 MCP 占位声明。报告与提议只发布到本 Run 的 Artifact，不修改数据目录、项目、Graph、Plugin 或协议。提议中的质量下降被 gate 拦截；质量未知仍是待验证提议，不能写成已经改善。

`reads` 是 Graph 作者接口和注意力约定；当前 Host 会传递上游 Artifact，MCP 也没有按信号限制查询权限。因此“一条信号一支”并非文件或工具级访问控制。P7 以实际工具轨迹核查遵守情况；需要更强隔离时应另定 Host 输入与 MCP 查询契约。
