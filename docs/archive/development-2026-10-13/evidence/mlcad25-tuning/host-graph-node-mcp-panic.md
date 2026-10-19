# Graph 节点挂载插件 MCP server：Host worker panic（交给 pilot/工具可见性那条线）

日期：2026-10-12 · 提出方：MLCAD 实验（隔离实例 8078） · 状态：**已修复并实测通过**（2026-10-12 22:06 主树 `tool_host.rs` 改动后复测）

## 复测结论（2026-10-12 23:19）

用**当前主树**重新构建的 Host（sha256 前缀 `3c46154c…`）重跑同一张图：**panic 0 次**，Run `completed`。沙箱内 AgentNode 通过插件 MCP 工具完成读—写—复读全链路：

```
kb_stats      → 8 claims（写前）
kb_add        → added SMOKE1 (status=hypothesis, revision=1)
kb_get        → [SMOKE1] hypothesis · smoke · rev1
```

数据落在插件声明的 mutable 目录（沙箱内 `/plugins/mlcad25-kb/data`），跨调用持久；测试条目随后被 `kb_update` 置为 `retired`，知识库现有 9 条。

复测期间还暴露两个**部署侧**（非本 bug）前置条件，已在本实验 launcher 中修正：
- 工作树要求保留继承的 `ANCHOR_DEPENDENCIES_PYTHON`，否则 `Pilot Python is enabled without an interpreter`。
- 插件清单里声明的 MCP server 命令，其 basename 必须出现在 `ANCHOR_RUNNER_ALLOWED_COMMANDS` 中（本实验写的是 `/usr/bin/python3.12`，需把 `python3.12` 加进允许命令），否则报 `command 'python3.12' is not authorized by the host sandbox policy`。**这条值得那条线判断是否为预期语义**：插件自己声明的 server 命令是否应当由插件声明授权，而不是依赖操作员的全局命令白名单。

## 修复前的问题描述（保留）


## 一句话

在工作树（含未提交改动，即 `git status` 里 `rust/anchor-runner-host/src/node_host.rs` 那 645+/283−）构建的 Host 上，**第一个挂载了带 `mcpServers` 插件的 Graph AgentNode** 一启动就让一个 tokio worker panic，节点任务随之死亡；在 HEAD（`4307d27`）构建的 Host 上同一份图不会 panic，而是被干净拒绝（该能力尚未实现）。

## 触发条件（可复现）

1. 插件 `mlcad25-kb`（本次实验新增，`plugin.json` 里 `mcpServers.kb` 为 stdio server，命令 `/usr/bin/python3.12 server.py`，另声明 `mutable: ["data/"]`）。插件本身独立验证通过（MCP 握手、`tools/list`、增删查改、证据规则），与本 panic 无关。
2. 一张最小图，单节点 AgentNode 挂该插件：

```json
{
  "objective": "Prove the dynamic knowledge base plugin is reachable from an AgentNode",
  "entry": "curate",
  "agents": {"curator": {"model": "models.default", "wall_time_limit_seconds": 600, "writes": ["notes.md"]}},
  "ops": {},
  "nodes": [{"id": "curate", "agent": "curator", "plugins": ["mlcad25-kb"]}],
  "edges": []
}
```

3. `POST /graphs`（201）→ `POST /trigger`。

## 观测

Run 记录里状态在启动后 **90 毫秒**就冻结，之后再无推进：

```
started: 2026-10-12T13:52:32.747Z
updated: 2026-10-12T13:52:32.836Z
status:  running   （此后一直 running，无 goose-acp fact，未进入模型调用）
```

同一秒 Host 日志（stderr）：

```
thread 'tokio-rt-worker' (37649) panicked at anchor-runner-host/src/node_host.rs:806:18:
`async fn` resumed after completion
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

`node_host.rs:806:18` 是 `tools()` 里的 `Box::pin(async move {`（工作树行号；HEAD 同行构造在 `node_host.rs:706`）。也就是说 panic 落在**工具解析**这一步，而插件 MCP server 的挂载就在这条路径上。

## HEAD 与工作树的差异（本次实验实测）

| | HEAD `4307d27` | 当前工作树（未提交） |
| --- | --- | --- |
| Graph 节点的插件 MCP 支持 | **无**：`node_host.rs` 里没有 `.mcp_servers`；节点提示词是 "Use only the anchor MCP tools" | 有：`node_host.rs:108`、`:429` 出现 `.mcp_servers` |
| 同一张图的实际行为 | 干净失败：`host execution interrupted: unsupported graph capability: MCP server \`mlcad25-kb\`` | **panic**，节点任务死亡，Run 卡在 running |

两次都是同一个插件、同一张图、同一个隔离 Host 配置，只换了二进制：
- 工作树二进制 sha256 前缀 `2fc8cd9c…`（本次实验 8078 的旧二进制）
- HEAD + 本实验自有改动的二进制 `aed4bba5…`（新 worktree `/var/tmp/anchor-mlcad`）

## 对这条线的价值

- 这是"Graph 节点支持插件 MCP"这条在制能力的**首次真实触发**记录的失败模式；HEAD 是"明确不支持"，不是崩溃，所以这不是既有回归，而是新能力的实现缺陷。
- 失败模式是 `async fn resumed after completion`（future 被二次 poll），发生在 `NodeHostResolver::tools()` 返回的 `ToolResolution`/内层 `async move` 上。建议优先检查这条 future 在引入插件 MCP 后被**构造或 poll 了两次**的路径（例如为拿工具清单而提前 poll、在 `select!` 里重复使用同一个 future、或把已完成的 future 存进结构后在另一处再 await）。
- 复现成本很低：装一个声明 `mcpServers` 的插件 + 一张单节点图即可；插件内容不影响结果（我们用的是自研 KB 插件，非 MCP 生态里的东西）。

## 我方（MLCAD 实验）的处置

- 动态知识库插件已就绪但**暂不在 Graph 节点启用**，等这条能力落地；期间知识库以只读快照 + 操作员维护的方式使用。
- 本实验的隔离二进制改为从 `/var/tmp/anchor-mlcad`（分支 `mlcad/devices-kb`，基线 HEAD）构建，只含本实验自有改动，不再混入未提交的在制品。
- 未改动主树里任何不属于本实验的内容；未 reset / checkout / clean。

## 2026-10-12 修复与验收补记（A217）

以上为原始报告与当时处置，保留不改。当前状态：**工作树已修复并完成隔离验收，尚未更新开发或 MLCAD 部署**。

### 根因与修复

二次 poll 的定位正确，但重复等待发生在 `rust/anchor-runner-host/src/goose_acp.rs` 的调用者，而不是 `NodeHostResolver::tools()` 主动重复构造工具。原代码用 `transport::bounded(&mut resolving, ...)` 等待工具准备；该调用既能返回 resolver 已结束的错误，也能返回外层取消/超时。统一错误分支随后又 `resolving.await`，普通错误因此让已经结束的 async future 被再次 poll，触发报告中的 panic，绕过既有 Run 错误结算。

修复通过私有 `resolve_node_tools` 将解析结果与外层等待结果分开：已结束的解析错误直接保留类型返回；只有外层取消/超时才通知未结束的工具准备，并等待其清理所拥有的 MCP 子进程。继续使用现有执行错误路径保存 failed，不新增 panic 兜底或恢复引擎，也不扩大命令授权。HEAD 的干净拒绝仍是历史上不支持该能力的表现，原“新能力实现缺陷，而非既有回归”的结论不变。

### 实际验证

- 修前最小回归得到相同 `node_host.rs:806:18` panic、Run active/running，命令退出 101；`/tmp/anchor-node-mcp-panic-red.log` 与保留 fixture `/tmp/.tmpiR9Cwk`。
- 修后 Goose 接入单测 **9 passed**，包含普通错误不重 poll/保留错误类型，以及取消、超时通知并等待清理；`/tmp/anchor-node-mcp-panic-unit.log`。
- 实际 Host → GraphRunner → NodeExecutionPort → Goose ACP → Bubblewrap 的错误回归：未授权 MCP 命令与 stdio 握手失败均保存 failed、active=false、无 panic、ready=200，没有模型请求。仅模型传输替换为确定性本地 Provider；证据 `/tmp/anchor-node-mcp-panic-green-evidence/109456/`。
- 实际 stdio echo Plugin 正常调用、产物及原生历史核对通过，**1 passed**；`/tmp/anchor-node-mcp-panic-success-02.log` 与 `/tmp/anchor-node-mcp-panic-green-evidence-02/110581/goose-graph-mcp-stdio-success/evidence.json`。整组第一次运行曾被新测试 fixture 的空 route/printf 转义阻断，原失败 `/tmp/anchor-node-mcp-panic-green.log` 保留，未将其记为整组通过。
- 原 KB Plugin 的隔离真实 DeepSeek Flash 验收通过：Run **`rust-18ddd0c41d928226` completed**；模型实际调用 `mlcad25-kb-kb_kb_get`，返回随机 fixture claim，notes.md 产物与成功工具响应一致，claim 仍为 hypothesis/revision 1。Host 正常退出 0、监听关闭、无遗留子进程。证据 `.local/node-mcp-panic-acceptance/real-kb-c596900f6c03c6a6/{evidence.json,native-evidence.json,artifact-response.json,independent-check.json}`。

真实验收只预置原插件的 manifest/server/Skill（逐文件 hash 一致），在隔离 Library 写一条 fixture，不读取或复制原 `data/kb.sqlite3`。Host 的共享解释器精确配置 `/usr/bin/python3.12`，Pilot 作者 Python 关闭；未为插件放行任意 Python 路径。模型曾提交两个不合法的辅助工具参数，得到明确拒绝后自行修正，最终按既有 receipt 完成。它不代表 Pilot 安装验收或完整 MLCAD 业务已验收。

冻结 Host（独立副本，nlink=1）SHA256：

```text
f55d8f05c6172466b94eacb0dda2b9a3d2cf26a4293cb5f7f1f7be3a136eae7f
```

固定 Goose SHA256：

```text
71e76c412597b2ecd96ed20d0706e7666f31c018216e7cb5d65c5ca5c44824a7
```

验收命令：

```bash
PATH=/home/mansteinl/.cargo/bin:$PATH cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --bin anchor-runner-host --locked --offline goose_acp::tests::
PATH=/home/mansteinl/.cargo/bin:$PATH ANCHOR_GOOSE_BINARY=/home/mansteinl/Anchor/.local/rust/bin/goose ANCHOR_TEST_EVIDENCE_ROOT=/tmp/anchor-node-mcp-panic-green-evidence cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --test goose_plugin_mcp --locked --offline -- --ignored --test-threads=1 --nocapture
PATH=/home/mansteinl/.cargo/bin:$PATH ANCHOR_GOOSE_BINARY=/home/mansteinl/Anchor/.local/rust/bin/goose ANCHOR_TEST_EVIDENCE_ROOT=/tmp/anchor-node-mcp-panic-green-evidence-02 cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --test goose_plugin_mcp --locked --offline graph_agent_calls_its_sandboxed_stdio_plugin_and_publishes_an_artifact -- --ignored --exact --nocapture
python3 -B .local/node-mcp-panic-acceptance/verify_kb.py
PATH=/home/mansteinl/.cargo/bin:$PATH cargo clippy --manifest-path rust/Cargo.toml -p anchor-runner-host --all-targets --locked --offline -- -D warnings
PATH=/home/mansteinl/.cargo/bin:$PATH cargo fmt --manifest-path rust/Cargo.toml --all -- --check
```

Clippy 与 workspace 格式检查均退出 0。真实验收脚本第一次误用 Library 配置字段，Graph 创建 HTTP 422，现场 `real-kb-ffdcdebaf48f8cfc/` 保留；修正验收配置后通过，未把脚本错误归为产品失败。集成回归需显式 `--ignored`，没有将默认跳过计为通过。

原实验实例、既有卡住 Run 和 `.env` 未改；本轮未部署、未提交。原卡住 Run 不会因为新代码自动结算，需按既有控制/恢复路径另行核查。
