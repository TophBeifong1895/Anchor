# M3 §10.18 合并交回

按 §10.17–§10.18 完成 master 同步及全部规定门槛。已合入 `dfd3427`，本分支可供 master 审查后快进；没有修改 master 工作区，没有启动 M4/M5。S27 已批准，S4 同时结清。

实现及本轮测试绑定 merge 提交 **`cf23ea8408931707fa87e84ce9857de9fd859abd`**，父提交为 `376b256` 与 `dfd3427`。`376b256` 只提交任务书、S27/S4 工作记录和 overview。最终证据由后续文档提交交付；该提交相对 merge 的 `rust/`、`apps/web/` 零 diff，最终 hash 在交回消息中给出。

## 结果

| 门槛 | 实际结果 |
| --- | --- |
| workspace | **132 targets，2,197 / 0 / 126**，退出 0。相比 §10.15 增加 44 个普通通过、1 个 ignored。 |
| Web test | **42 文件，356 / 0**，退出 0；相比上轮增加 23 项。 |
| Web build | 退出 0，保留 Vite 已有的大 chunk 提示。 |
| Web 相关浏览器回归 | **10 / 0**：页面保留、创建重试、工作台、布局。隔离端口 5188、系统 Chrome，mock API。 |
| 固定 Goose 完整 Host ignored | **41 targets，123 / 0 / 0**，退出 0；没有失败豁免。 |
| 新增 canonical-account 独跑 | **1 / 0 / 0**，退出 0；完整套件再次通过。 |
| master 查询锁及正文读取回归 | 两项分别 **1 / 0 / 0**。 |
| master 助手身份回归 | **2 / 0 / 0**。 |
| workspace/all-target Clippy `-D warnings`、fmt check | 均退出 0。 |
| Run 访问围栏 | **1 / 0 / 0**，退出 0；围栏与 `0aa5197` 字节一致。 |
| C1 / 旧类型 / Session | Kernel 相对 `0859373` 零 diff；旧类型及控制轮询名称零命中；Session crate 与 `dfd3427` 字节一致。 |

所有 target 的原样 `test result`、命令、退出码、日志路径与 SHA256 在 [merge-checks.json](merge-checks.json)。原始日志仅在本机 `/tmp/anchor-m3/section-10-18/`，没有提交 `.log` 文件。

准备阶段有三项需要如实区分：Host 包没有 lib target，首个 `--lib --no-run` 命令退出 101，随后按 bin 编译成功；第一次锁回归误用部分名字加 `--exact`，实际零项，不计通过证据，随后正确执行一项；首个 Web build 因 master 新夹具仍写旧 `state.status` 而类型检查失败，改用已有 waiting 夹具后完整 356 项与 build 均通过。未删业务断言或用忽略测试消除失败。

## 冲突、专项审计与新增测试

[逐项冲突登记与锁/哈希审计](merge-conflicts.md) 覆盖三个内容冲突及自动合并后的语义适配：

- `ChannelSessions.tsx` 合并 master 恢复阻止原因和本分支 Kernel 词汇；`architecture.md` 合并页面/渠道事实及 Kernel 边界。
- `run_access/projection.rs` 保留本分支实现，不移植旧 pause/cancellation 执行器表。
- **(a)** 每个 `KernelRuns.active` 生产锁点已逐组登记；没有持锁读取平台 Session、部署或 Plugin，也没有跨 `.await` 持锁。保留 master 回归，在部署 snapshot 回调中直接断言 Kernel registry 可锁。
- **(b)** 合并前本分支投影仅核对 Session current binding，不查部署或哈希正文；渠道 status 仍使用完整 definition。合并 master 新增的部署资格时，同时保留轻量观测修复：先排除历史绑定，观测使用 live `PluginMetadata`，配置/启用/执行前完整检查保留。inotify 回归证明查询不打开正文，同时授权变化与资源消失立即生效。
- Session v9、渠道账号别名、创建幂等采用 master 业务实现，本分支不改变 schema。

master 新增 **45** 个 Rust 测试的函数名均保留，清单见 [merge-test-inventory.json](merge-test-inventory.json)。Goose 相比上轮 122 新增且只新增：

`host_restart_recovers_only_the_canonical_account_and_preserves_legacy_history`

该用例走 Kernel，保留 canonical 唯一恢复、旧账号历史/产物不变、原生 Session 延续、工作区继承、恢复零模型请求、后续消息复用及退役实例不重启等断言。master 新测试中的旧 running/stopped/cursor 改读 Executing/Waiting(Human)/frontier；没有新增旧状态兼容层。

## 固定二进制与执行范围

Cargo 产物使用 `/tmp/anchor-goose-regression-target-core`，relay 同目录。workspace/Clippy 后预编译最终单包测试命令，再冻结 Host、relay、sandbox-network、wecom-tools、docmost-tools、MCP http_fixture 和固定 Goose 七个文件；独跑与完整套件后 SHA256 全部不变，见 [merge-binary-identities.json](merge-binary-identities.json)。Goose 固定摘要为 `17e065a79cb0508413af714724aed51d6b76972dda9feebdda701330134fb640`。

完整实际命令（环境与逐 target 输出也在 checks JSON）：

```sh
CARGO_TARGET_DIR=/tmp/anchor-goose-regression-target-core \
CARGO_BUILD_JOBS=2 \
ANCHOR_GOOSE_BINARY=/var/tmp/anchor-goose-build/artifacts/goose-acp.first \
ANCHOR_GOOSE_LOCAL_NETWORK=1 \
ANCHOR_GOOSE_RELAY_BINARY=/tmp/anchor-goose-regression-target-core/debug/anchor-net-relay \
ANCHOR_TEST_RUST_TOOLCHAIN=stable-x86_64-unknown-linux-gnu \
ANCHOR_PILOT_RUSTUP=/home/mansteinl/.cargo/bin/rustup \
ANCHOR_PILOT_RUSTUP_HOME=/home/mansteinl/.rustup \
ANCHOR_TEST_EVIDENCE_ROOT=/tmp/anchor-m3/section-10-18/goose-full-evidence \
cargo +stable test --manifest-path rust/Cargo.toml -p anchor-runner-host \
  --locked --no-fail-fast -- --ignored --test-threads=1
```

这里的真实 Goose 使用确定性本地 Provider，实际经过 Host、ACP、授权 MCP、Sandbox 与 Artifact；真实模型 Provider、外部渠道实网、部署未执行。没有修改或清理开发数据。

## §9 交回

| 项 | 结论 |
| --- | --- |
| 里程碑 / commits | M3 合并验收；`376b256` 文档批准，`cf23ea8` 合入 master `dfd3427`；最终文档提交见交回消息。 |
| C2 | save **0**，mutation **0**；原 [52 项去向](c2-destinations.md) 仍有效，没有恢复旧 Run 写入入口。 |
| 对照 | 本轮不重做 M2；workspace 包含既有 30,000 例冻结快照对照，快照先于删除的不变量通过。 |
| S 候选 | 新增 **0**；S27 已批准并结清 S4。 |
| 断言修改 / 删除测试 | 本轮 **3 类**机械适配：锁夹具改真实 Kernel，canonical 状态/frontier，Web waiting 夹具。删除 **0**，既有测试新增 ignore **0**，业务断言放宽 **0**。 |
| 组件归属 | Session/渠道业务规则仍归原事实所有者；projection 和 live metadata 属可替换 Host 边缘；新增 registry helper 仅测试可用，不增运行时机制。 |
| 核心与极限 | Kernel 零改动，不引入与模型、规模、时间或入口相关的核心分支；三条件及四极限的最终逐条度量仍属未开始的 M5，不能以本轮集成回归替代。 |
| 停止条件 | 未新增触发；此前恢复冲突已有 §10.15 裁定。 |
| master 包含关系 | `git merge-base --is-ancestor dfd3427 HEAD` 退出 0。审查通过后按 §10.17 由 master 一侧快进，本分支不改其工作区。 |
| 未关闭项 | W4、真实模型 Provider、外部渠道实网、部署未验；M4/M5 未开工。 |
