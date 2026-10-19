# M1.5 验证

按任务书 §10.8 完成两次同步：先以 `c6454c3` 合并 `master@2b3b7a2`，再仅合并 F0 `e3a5778`。原生 Run 模块归入 `application/run_access/`，保留 M1 独立 pause 与三相位语义；未合入 `06f7b8a` 及后续 F1，M2 尚未开始。本轮按里程碑交回。规则清单见 [measurement.md](measurement.md)，位点与源文件摘要见 [counts.json](counts.json)，完整机器结果见 [checks.json](checks.json)。

## 已执行检查

| 检查 | 结果 | 原始输出 |
| --- | --- | --- |
| `cargo build -p anchor-sandbox-bwrap --bins` | 通过 | `/tmp/anchor-m15/sandbox-build.log` |
| `cargo fmt --all -- --check` | 通过 | `/tmp/anchor-m15/f0-fmt-final.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | `/tmp/anchor-m15/f0-clippy-final.log` |
| `cargo test --workspace --no-fail-fast` | 2023 通过 / 0 失败 / 122 既有忽略；127 个 target | `/tmp/anchor-m15/f0-workspace.log` |
| `cargo test -p anchor-runner-host --test run_access_fence -- --nocapture` | 1 通过；153 个生产文件，business 192 / runtime 93 | `/tmp/anchor-m15/f0-fence-final.log` |
| `cargo test -p anchor-runner-host run_status_http_json_bytes_match_the_phase_contract -- --nocapture` | 1 通过；十种旧存储状态，原始 HTTP 字节比较 | `/tmp/anchor-m15/f0-json.log` |
| `npm --prefix apps/web test` | 37 文件、311 通过 | `/tmp/anchor-m15/web-test-final.log` |
| `npm --prefix apps/web run build` | 通过；既有 Vite chunk 提示 | `/tmp/anchor-m15/web-build.log` |
| `ANCHOR_BROWSER_BINARY=/usr/bin/google-chrome ANCHOR_WEB_TEST_PORT=5195 npm --prefix apps/web run test:e2e -- message-management.spec.ts` | 桌面 1440 / 手机 390，两项通过 | `/tmp/anchor-m15/web-e2e-final.log` |
| `bash scripts/check-sensitive-wording.sh` | 通过 | 本机终端输出 |
| `git diff 6ee2b0e --stat -- rust/anchor-kernel` | 空，C1 通过 | `/tmp/anchor-m15/kernel-diff.txt` |

Rust 命令在 `rust/` 下执行。生产 GraphValidator 相对 M1 无改动。workspace 相对 M1 审查值 1922 / 0 / 115 增加 101 项通过、7 项继承的忽略；本轮没有删除测试或新增 ignore。Web 检查在首次合并后执行；F0 未修改 Web，沿用该结果。

围栏测试与上游 `e3a5778` 字节一致，SHA256 为 `c8c6878e467d308a3fe4057c98a03743f914d2e2c0e54b90b90f9bfa01c91923`；未扩大 `runtime_file` 范围。业务引用上限 224 → 192，仅降低 api.rs 14 → 3、api/runs.rs 3 → 2、conversations.rs 27 → 15、liveness.rs 6 → 2、run_projection.rs 6 → 2，其他文件上限未提高。继承的 F0 HTTP 字节测试按已批准的新词汇适配，十个状态样本保留。断言登记共 21 处 Rust 与 1 处 Web fixture，新增 S 候选 0。

## Goose 命令与最终全量结果

实际经过 Host、Goose ACP、Sandbox 和 MCP，只替换模型传输为本地确定性 Provider；这些结果不代表真实模型 Provider 验收。二进制使用前以 `sha256sum` 核对为 `17e065a79cb0508413af714724aed51d6b76972dda9feebdda701330134fb640`。

按任务书 §10.9 设置本地网络与 relay，`anchor-net-relay` 来自与 Host 同次构建；最终套件在 G0 测试收束后执行。完整命令：

```bash
cd rust
export ANCHOR_GOOSE_BINARY=/var/tmp/anchor-goose-build/artifacts/goose-acp.first
export ANCHOR_GOOSE_LOCAL_NETWORK=1
export ANCHOR_GOOSE_RELAY_BINARY=/home/mansteinl/code/anchor-constitution-core/rust/target/debug/anchor-net-relay
export ANCHOR_TEST_RUST_TOOLCHAIN=stable-x86_64-unknown-linux-gnu
export ANCHOR_PILOT_RUSTUP=/home/mansteinl/.cargo/bin/rustup
export ANCHOR_PILOT_RUSTUP_HOME=/home/mansteinl/.rustup
export ANCHOR_NATIVE_PLUGIN_BIN_DIR=/home/mansteinl/code/anchor-constitution-core/rust/target/debug
export ANCHOR_TEST_EVIDENCE_ROOT=/tmp/anchor-m15/f0-goose-evidence
cargo test -p anchor-runner-host --locked --no-fail-fast -- --ignored --test-threads=4
```

单次完整执行 `/tmp/anchor-m15/f0-goose.log`：**117 通过 / 2 失败 / 0 忽略，40 个 target**，退出码 101。下表保留每个 target 的原样结果行，未拼接复跑结果。

| target | 原样结果 |
| --- | --- |
| `unittests src/bin/anchor-net-relay.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `unittests src/main.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 943 filtered out; finished in 31.08s` |
| `tests/channel_startup.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s` |
| `tests/goose_acp.rs` | `test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 33.51s` |
| `tests/goose_channel.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 9.88s` |
| `tests/goose_compaction.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 10.35s` |
| `tests/goose_completion.rs` | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 23.24s` |
| `tests/goose_conversation.rs` | `test result: FAILED. 12 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 53.61s` |
| `tests/goose_development_environment.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 19.95s` |
| `tests/goose_distribution.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 85.40s` |
| `tests/goose_elicitation.rs` | `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 20.71s` |
| `tests/goose_library.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 8.13s` |
| `tests/goose_media.rs` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 18.81s` |
| `tests/goose_model_settings.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 8.67s` |
| `tests/goose_native_activation_spike.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 1.41s` |
| `tests/goose_persistent_assistant.rs` | `test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 38.65s` |
| `tests/goose_pilot.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 8.08s` |
| `tests/goose_pilot_attachments.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 11.44s` |
| `tests/goose_pilot_authoring.rs` | `test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 11.08s` |
| `tests/goose_pilot_compaction.rs` | `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 10.15s` |
| `tests/goose_plugin_configuration.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 55.78s` |
| `tests/goose_plugin_disclosure.rs` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 4.28s` |
| `tests/goose_plugin_mcp.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 21.34s` |
| `tests/goose_plugin_summary.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 9.29s` |
| `tests/goose_plugin_turns.rs` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 25.49s` |
| `tests/goose_provider_errors.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 14.56s` |
| `tests/goose_responses.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 9.35s` |
| `tests/goose_session_calls.rs` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 13.93s` |
| `tests/goose_trace.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 8.80s` |
| `tests/local_inputs.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s` |
| `tests/local_network.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s` |
| `tests/multinode.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `tests/native_plugins.rs` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 27.49s` |
| `tests/parallel.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `tests/routing.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s` |
| `tests/run_access_fence.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s` |
| `tests/standalone_graph_call.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s` |
| `tests/standalone_library.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s` |
| `tests/standard_host.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s` |
| `tests/writer_lease.rs` | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s` |

## master 基线对照

采用任务书 §10.9 已验收的 G0 基线：基点 `e3a5778`，完整忽略测试 116 通过 / 3 失败 / 0 忽略。G0 relay 修复提交为 `250435d54510230b63dd64521bff9614c3a0bef1`，尚未合入本分支。证据目录 `/var/tmp/anchor-g0/.local/goose-baseline-20261019/` 中的 `baseline.json`、`baseline-table.md`、`full-ignored.log`、`confirmed-retries.json` 与 `confirm-*.log` 是 G0 交回时的原始证据入口；本次最终收尾复核时该目录已不可获取，`checks.json` 保留交回的完整 target 摘要和独立确认记录，未据其他旧日志替代。独立确认记录 `overlap=false`。早期负载重叠复跑位于 `retries-under-load/`，不作为最终定性依据。

| 用例 | 分支最终全量 | G0 全量及独立确认 | 定性 |
| --- | --- | --- | --- |
| `next_conversation_invocation_changes_model_and_endpoint_without_losing_history` | 超时等待 `completed` | 同样超时；两边旧 Provider 6 请求、新 Provider 0 请求 | master 既有失败；fixture 未更新已保存的模型配置 |
| `candidate_declaration_cannot_enable_network_or_credentials_and_command_cancel_can_continue` | 第 577 行空列表断言失败，出现无效默认 network 挂载对象 | 同一行、同一对象失败 | master 既有失败；业务断言未改 |
| `extracted_goose_runtime_resumes_without_sources_or_replaying_workspace_effects` | 通过 | 独立确认仍失败；暂存 helper 为硬链接，被 builder 拒绝 | G0 已确认的基线残留，分支最终通过 |

最终两项失败均在基线名单中，**新增失败 0**，达到本轮相对基线门槛；不能写成 Goose 全绿。

## 前序执行与证据范围

首次合并 `c6454c3` 的 workspace 为 2021 / 0 / 122。该阶段带 relay 的 Goose 全量为 112 / 7 / 0，39 个 target；其中五项是旧协议适配遗漏：助手 action 回执 `asked` → `operation` 两项，以及 Session Call 共享轮询补上 Waiting/Child 三项。修正后两个完整 target 顺序复跑 12 + 4 = 16 / 0 / 0。完整首轮、复跑的原样 target 结果与失败名单保存在 `checks.json` 的 `goose.pre_f0`，原始输出分别为 `/tmp/anchor-m15/goose-relay.log`、`/tmp/anchor-m15/goose-retry-final.log`。最终 F0 全量已经重新覆盖这些测试。

最早无 relay 的运行已中止并按 §10.9 作废。一次旧复跑与 G0 后启动的测试重叠后主动中止，只采用双方收束后的完整执行。F0 定向 run_access 运行完成 16 项后，中止了重复的 1000 seed 长用例，该中止命令不记为通过；最终 workspace 已完整覆盖。临时 master-base worktree 只编译未跑，采用 G0 基线后已移除。

原始输出与运行产物保留在 `/tmp/anchor-m15/`，最终 Goose 产物位于 `f0-goose-evidence/`。本轮不提交原始 `.log`；`checks.json` 保存所引用原始输出的 SHA256，以便核对。

## 未关闭边界

M2 原生能力、M3 切换与删除旧 Runner、M4/M5 收尾均未开始；完整 F1 以后按 master 接口一次性对齐，M3 仍以 F2 合入为前置。旧 S23 显示可恢复但旧写入可能拒绝的过渡边界按用户批准保留。真实模型 Provider、生产数据离线迁移与 W4 未关闭。
