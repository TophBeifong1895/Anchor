# M1.5 验证

当前快照合并 `master@2b3b7a2`；M1 独立 pause 与新相位语义保留。F0 尚待按任务书 §10.8 单独合并，未开始 M2。规则清单见 [measurement.md](measurement.md)，机器摘要见 [checks.json](checks.json)。

## 已执行检查

| 检查 | 结果 | 原始输出 |
| --- | --- | --- |
| `cargo build -p anchor-sandbox-bwrap --bins` | 通过 | `/tmp/anchor-m15/sandbox-build.log` |
| `cargo fmt --all -- --check` | 通过 | `/tmp/anchor-m15/fmt-before-merge-commit.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | `/tmp/anchor-m15/clippy-before-merge-commit.log` |
| `cargo test --workspace --no-fail-fast` | 2021 通过 / 0 失败 / 122 既有忽略；126 个 target 汇总 | `/tmp/anchor-m15/workspace.log` |
| `npm --prefix apps/web test` | 37 文件、311 通过 | `/tmp/anchor-m15/web-test-final.log` |
| `npm --prefix apps/web run build` | 通过；既有 Vite chunk 提示 | `/tmp/anchor-m15/web-build.log` |
| `ANCHOR_BROWSER_BINARY=/usr/bin/google-chrome ANCHOR_WEB_TEST_PORT=5195 npm --prefix apps/web run test:e2e -- message-management.spec.ts` | 桌面 1440 / 手机 390 两项通过 | `/tmp/anchor-m15/web-e2e-final.log` |
| `bash scripts/check-sensitive-wording.sh` | 通过 | 本机终端输出 |
| `git diff 6ee2b0e --stat -- rust/anchor-kernel` | 空，C1 通过 | `/tmp/anchor-m15/kernel-diff.txt` |

生产 GraphValidator 相对 M1 无改动。workspace 相对 M1 审查值 1922/0/115 增加 99 项通过、7 项既有忽略，来自 master 新测试；本次没有删除测试或新增 ignore。原始日志不入库。

## Goose 命令与结果

实际经过 Host、Goose ACP、Sandbox 和 MCP，只替换模型传输为本地确定性 Provider；这些结果不代表真实模型 Provider 验收。二进制使用前以 `sha256sum` 核对为 `17e065a79cb0508413af714724aed51d6b76972dda9feebdda701330134fb640`。

```bash
cd rust
export ANCHOR_GOOSE_BINARY=/var/tmp/anchor-goose-build/artifacts/goose-acp.first
export ANCHOR_GOOSE_LOCAL_NETWORK=1
export ANCHOR_GOOSE_RELAY_BINARY=/home/mansteinl/code/anchor-constitution-core/rust/target/debug/anchor-net-relay
export ANCHOR_TEST_RUST_TOOLCHAIN=stable-x86_64-unknown-linux-gnu
export ANCHOR_PILOT_RUSTUP=/home/mansteinl/.cargo/bin/rustup
export ANCHOR_PILOT_RUSTUP_HOME=/home/mansteinl/.rustup
export ANCHOR_NATIVE_PLUGIN_BIN_DIR=/home/mansteinl/code/anchor-constitution-core/rust/target/debug
export ANCHOR_TEST_EVIDENCE_ROOT=/tmp/anchor-m15/goose-relay-evidence
cargo test -p anchor-runner-host --locked --no-fail-fast -- --ignored --test-threads=4
# 修正旧协议断言后，使用 goose-retry-final-evidence 目录顺序复跑：
cargo test -p anchor-runner-host --locked --test goose_persistent_assistant --test goose_session_calls --no-fail-fast -- --ignored --test-threads=1
```

完整首轮 `/tmp/anchor-m15/goose-relay.log`：112 通过 / 7 失败 / 0 忽略，39 个 target。下表原样保留每个 target 的结果行，不把复跑拼接成一次全量成功。

| target | 原样结果 |
| --- | --- |
| `unittests src/bin/anchor-net-relay.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `unittests src/main.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 942 filtered out; finished in 31.06s` |
| `tests/channel_startup.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s` |
| `tests/goose_acp.rs` | `test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 33.27s` |
| `tests/goose_channel.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 10.30s` |
| `tests/goose_compaction.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 10.90s` |
| `tests/goose_completion.rs` | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 24.79s` |
| `tests/goose_conversation.rs` | `test result: FAILED. 12 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 55.45s` |
| `tests/goose_development_environment.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 28.09s` |
| `tests/goose_distribution.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 86.70s` |
| `tests/goose_elicitation.rs` | `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 20.96s` |
| `tests/goose_library.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 8.10s` |
| `tests/goose_media.rs` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 18.89s` |
| `tests/goose_model_settings.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 10.96s` |
| `tests/goose_native_activation_spike.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 1.75s` |
| `tests/goose_persistent_assistant.rs` | `test result: FAILED. 10 passed; 2 failed; 0 ignored; 0 measured; 6 filtered out; finished in 38.73s` |
| `tests/goose_pilot.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 8.40s` |
| `tests/goose_pilot_attachments.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 11.71s` |
| `tests/goose_pilot_authoring.rs` | `test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 11.25s` |
| `tests/goose_pilot_compaction.rs` | `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 10.25s` |
| `tests/goose_plugin_configuration.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 55.60s` |
| `tests/goose_plugin_disclosure.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 4.45s` |
| `tests/goose_plugin_mcp.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 21.34s` |
| `tests/goose_plugin_summary.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 9.30s` |
| `tests/goose_plugin_turns.rs` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 25.11s` |
| `tests/goose_provider_errors.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 15.05s` |
| `tests/goose_responses.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 9.24s` |
| `tests/goose_session_calls.rs` | `test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 6 filtered out; finished in 10.96s` |
| `tests/goose_trace.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 8.78s` |
| `tests/local_inputs.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s` |
| `tests/local_network.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s` |
| `tests/multinode.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `tests/native_plugins.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 26.47s` |
| `tests/parallel.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `tests/routing.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s` |
| `tests/standalone_graph_call.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s` |
| `tests/standalone_library.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s` |
| `tests/standard_host.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `tests/writer_lease.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s` |

复跑 `/tmp/anchor-m15/goose-retry-final.log`：

| target | 原样结果 |
| --- | --- |
| `tests/goose_persistent_assistant.rs` | `test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 136.43s` |
| `tests/goose_session_calls.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 44.75s` |

首轮五项失败是测试协议适配遗漏：助手 action 回执 `asked` 已改为 `operation`（2 项）；Session Call 的共享轮询补上 Waiting/Child（3 项）。原 HTTP 202、动作身份、Session 投递、产物、成本、权限和不重放断言保留，登记在工作记录 M1.5。

## master 基线对照

优先使用 G0 已完成的全量基线及独立复跑：`/var/tmp/anchor-g0/.local/goose-baseline-20261019/{baseline.json,baseline-table.md,full-ignored.log,retry-*.log}`。基点 `e3a5778`，生产源码与当前合并输入 `2b3b7a2` 相同；F0 仅增加围栏、字节测试与文档。G0 的 fixture 改动为 relay 的环境预检/显式传递，没有放宽业务断言。master 全量 116 通过 / 3 失败 / 0 忽略。

| 用例 | 分支结果 | G0 全量与独立复跑 | 定性 |
| --- | --- | --- | --- |
| `next_conversation_invocation_changes_model_and_endpoint_without_losing_history` | 超时 `completed` | 同样超时 | master 既有失败 |
| `candidate_declaration_cannot_enable_network_or_credentials_and_command_cancel_can_continue` | 已挂载列表出现无效的默认 network Plugin | 同一行、同一对象内容失败 | master 既有失败；业务断言未改 |
| `extracted_goose_runtime_resumes_without_sources_or_replaying_workspace_effects` | 通过 | master 缺少原生 Plugin 包，独立复跑仍失败 | master 环境边界；分支已构建并设置原生 Plugin 目录 |

修正后的两个 target 全部通过时，按每个 target 的最新完整执行计算为 117 通过 / 2 失败 / 0 忽略；两项失败均在 master 基线中，新增失败为 0。该值是覆盖合并值，并非单次全量输出。

最早没有 relay 的运行已中止；控制面 loopback 在 slirp 隔离下不可达，导致扩展初始化 `-32603`，不计为迁移回归。一次复跑遇到 G0 新启动独立复跑后主动中止，仅以双方收束后的 `goose-retry-final.log` 为准。所有中止输出保留在 `/tmp/anchor-m15/`。临时 master 干净 worktree 只完成测试编译；采用 G0 完整基线后不再执行重复基线。

## 未关闭边界

F0 同步、围栏与新词汇字节适配尚未执行；M2 原生能力、M3 切换和删除旧 Runner、M4/M5 收尾均未开始。旧 S23 显示可恢复但旧写入可能拒绝的过渡边界按用户批准保留。真实 Provider、生产数据离线迁移与 W4 未关闭。
