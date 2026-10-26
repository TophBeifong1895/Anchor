# H5 验证交回

2026-10-26；基点 `fcda48c87f1555cb2238b149e96cc2b166e92821`，盘点提交 `996b29e`，分支 `host/h5-reverse-deps`，独立 worktree `/tmp/anchor-host-h5`。按架构验收者的两层裁决实施；开发与规定检查完成，待架构验收。

## 结果与计数口径

| 项目 | H4 基线 | 本次实际结果 |
| --- | ---: | ---: |
| 服务 → 用例实际引用 | 22 | 20（消除 durable cleanup、scope 释放各 1） |
| 服务 → 入口适配实际引用 | 23 | 14（工具名 7、回复图片读取 1、Session workspace 1 已消除） |
| 用例 → 入口适配实际引用 | 1 | 1（C01 按裁决保留） |
| 已登记的命名例外引用 | 0 / 0 / 0 | 20 / 14 / 1 |
| 尚未登记命名例外的引用 | 22 / 23 / 1 | 0 / 0 / 0 |
| Run 围栏业务 / 运行时引用 | 0 / 56 | 0 / 56 |
| Plugin 所有权围栏外部类型 | 0 / 0 | 0 / 0 |
| Host 生产物理行数 | 51,256 | 51,251（净减 5，严格低于基线） |

实际消除 11 处引用；剩余 35 处属于 13 个文件的命名架构欠债。0 / 0 / 0 指未登记的引用，不把命名处理记成实际依赖消除。层次分类不变，逐文件数字只降不升，命名项仍受原来的计数上限约束；未知文件或未知命名标识不能取得例外。

行数沿用既有 `experiments/constitution-host-preflight/final/measure_lines.py::test_partition`，包括生产所属空行和注释，排除其识别的测试。没有以代码迁移的删除行重复计算收益，也没有调整分区规则。基线及逐文件数据见 [baseline-lines.json](baseline-lines.json)，交回数据见 [final-lines.json](final-lines.json)。Host src tree 为 `ab58f992db81573f9c136210ad2dbfb33a021cec`；最终提交对应同一源码树。

## 实际执行的检查

所有测试命令均带 `CARGO_TARGET_DIR=/home/mansteinl/Anchor/rust/target XDG_RUNTIME_DIR=/run/user/1001`；check/clippy 使用相同的 `CARGO_TARGET_DIR`。测试走 tmpfs，复用已有 anchor-sandbox-network，未发生缺少该工具导致的沙箱网络误报。

| 命令 | 结果与证据 |
| --- | --- |
| `cargo +stable check --manifest-path rust/Cargo.toml -p anchor-runner-host --all-features --all-targets --locked` | 最终 exit 0；首轮发现 API 回复读取路径遗漏，以及 G14 的生产调用，修正路径并保留安装入口后通过 |
| `scripts/test.sh -p anchor-runner-host --bin anchor-runner-host durable_write:: --locked` | exit 0；9 passed，[输出](durable-tests.log) |
| `scripts/test.sh -p anchor-runner-host --locked` | 修复 clippy 前、反向验证恢复后各执行一次；两次均 exit 0，最终 1,233 passed / 0 failed / 110 ignored，42 个测试目标；[最终输出](tests.log) |
| `scripts/test.sh -p anchor-runner-host --test layer_fence --test run_access_fence --test plugin_library_fence --test configuration_fence --locked -- --nocapture` | exit 0；7 passed / 0 failed；[输出及三类计数](fences.log)；最终全量再次包含这 7 项 |
| `cargo +stable clippy --manifest-path rust/Cargo.toml -p anchor-runner-host --all-features --all-targets --locked -- -D warnings` | 最终 exit 0；[输出](clippy.log)。[首轮](initial-clippy.log)因 main.rs 两处 needless_borrow 失败，删除多余借用后复跑通过 |
| `cargo +stable fmt --manifest-path rust/Cargo.toml --all -- --check` | exit 0 |
| `scripts/check-sensitive-wording.sh` | exit 0；最终纳入新增产物后再检查 |
| `python3 experiments/host-convergence/h5/measure-baseline.py <git-write-tree结果> final-lines.json` | exit 0；51,251，使用 test_partition；可在本提交上以 `HEAD` 代入重测 |
| `git diff 9a5fa57 -- rust/anchor-kernel` | 空（0 字节） |
| `git diff fcda48c -- rust/anchor-runner-host/run-access-fence.txt rust/anchor-runner-host/tests/plugin_library_fence.rs` | 空；Run 上限、Plugin 所有权检查均未改变 |
| `git diff --check`、`git diff --cached --check` | exit 0 |

110 ignored 如实保留，具体原因在测试输出；未把跳过项或真实 provider 端到端验收写成通过。本阶段没有新增真实 provider 验收。

## 反向验证及断言复核

在 `durable_write.rs` 末尾临时增加生产代码 `const _: usize = std::mem::size_of::<crate::application::RunApplication>();`，然后带上述测试环境执行：

```sh
scripts/test.sh -p anchor-runner-host --test layer_fence --locked -- --exact layer_fence --nocapture
```

测试按预期 exit 101，明确报 `durable_write.rs service_application 1 (maximum 0; ...)`，未登记计数变为 1 / 0 / 0。[完整失败输出](negative-fence.log)。使用 try/finally 写回最初读取的字节，确认逐字节相等；恢复前后原文件 SHA-256 均为 `e1f98b3187f1f0a57b1fe5eae9533b0d82d92c6188eaa78aec5ba26bcde264da`。恢复后再次执行完整 Host 测试通过，临时违例未进入提交。

`session_workspace`、`release_development_scope`、两个回复图片读取函数的正文与基点逐字节相同。清理回调仍在固定目录扫描之后才枚举工作区；枚举失败仍只增加 failed，原先的扫描/清理实现未改。

使用 [audit-assertions.rs](audit-assertions.rs) 复用围栏词法器、保留字符串字面量，仅归一化明确迁移的模块路径及格式化末尾逗号：

```sh
rustc --edition=2024 experiments/host-convergence/h5/audit-assertions.rs -o /tmp/anchor-host-h5-assertions
/tmp/anchor-host-h5-assertions /tmp/anchor-host-h5-assertions-base/rust/anchor-runner-host/src > /tmp/h5-before-assertions
/tmp/anchor-host-h5-assertions rust/anchor-runner-host/src > /tmp/h5-after-assertions
```

基点副本逐文件来自 `git show fcda48c:<src或tests路径>`。以 Python Counter 比对断言 token，多重集结果为源码 5,898 → 5,898，内容相同；integration 1,953 → 1,954，全部既有断言保留，新增 1 处用于验证获准命名标识。结果见 [assertion-audit.log](assertion-audit.log)。测试构造调整仅配合配置参数、reader 和清理回调注入；没有行为断言变更。辅助审计脚本的直接 `rustfmt +stable` 调用因命令未在 PATH 而未执行，随后用 `rustup run stable rustfmt --edition 2024 .../audit-assertions.rs` 完成格式化；工作区规定的 cargo fmt 检查通过。

## 被删除和替代的路径

- 删除 `RunApplication.host_configuration` 整份副本、同名 getter，以及 `check_store` 恒等 state 根比较分支；保留原 executor 检查、创建和 canonicalize 顺序。已有 state/catalog 根之外仅保存 Library 覆盖及 bwrap。
- 删除 `execution.rs` 内默认 reader 构造，改接收入口 reader；删除 `durable_write.rs` 生产侧的默认 application 构造，入口通过惰性回调提供既有工作区枚举。`node_host.rs` 默认 reader 按 G2 留存。
- 删除 `HostConfiguration::tool_environment` 的转发，唯一的可选 Library 加载入口归 `ToolEnvironment::for_library`；Pilot/工具装配接收已解析路径，保持只从显式 Library 根加载的规则。
- 删除只作配置转发的 `pilot_tools::pilot_plugin_sandbox`，复用原 `pilot_plugin_sandbox_with_environment`；`PilotTurnContext` 不再保存完整配置。
- 删除 `node_tools` / `channel_tools` 的五个工具名定义，唯一契约归 `tool_names`；全部调用改路径，无重导出。
- 删除 `channel_tools` 的回复图片读取与共享限制定义，原实现归 `channel_replies`；Node 结算和 API 投影调用事实所有者，校验顺序、路径及字节保持。
- 删除 `pilot_tools::session_workspace` 和 `application::development_dependencies::release_development_scope`，唯一实现归 `pilot_resources`，含 A22/B20 调用及测试均改路径，无兼容转发。
- 没有整文件删除；`plugin_library` 没有完全闲置的转发方法可删，详见下方 G14 偏离。

## 保留的命名例外

| 名称 | 文件 | 服务→用例 / 服务→适配 / 用例→适配 | 保留理由 |
| --- | --- | --- | --- |
| `G10-candidate-context` | `application/plugin_candidates.rs` | 0 / 0 / 1 | 随 G9 保留 PilotTurnContext，候选参数拆分延后 |
| `G1-assistant-run` | `assistant.rs` | 1 / 0 / 0 | 随 G2 保留 Run 交接查询，避免拆分在线 owner 语义 |
| `G1-assistant-plugins` | `assistant_plugins.rs` | 2 / 0 / 0 | 随 G2 保留 Run 授权投影与租约调用 |
| `G3-channel-management` | `channel_supervisor/management.rs` | 2 / 0 / 0 | 随 G8/G9 保留渠道用例契约与错误映射 |
| `G1-conversation-resources` | `conversation_resources.rs` | 2 / 0 / 0 | 随 G2 保留 Run 删除协调与保留引用检查 |
| `G8-goose-pilot` | `goose_acp/pilot.rs` | 0 / 3 / 0 | Goose 直接使用 Pilot 适配器；端口接线预算 +35 行，列为架构欠债 |
| `G8-pilot-interaction` | `goose_acp/pilot_interaction.rs` | 8 / 2 / 0 | 随 G8 保留原生交互的用例前置条件、错误与工具权限映射 |
| `G8-channel-interaction` | `goose_acp/pilot_interaction/channel_management.rs` | 0 / 2 / 0 | 随 G8 保留渠道适配参数解析，继续共用唯一实现 |
| `G1-local-inputs` | `local_inputs.rs` | 1 / 0 / 0 | 随 G2 保留 RunProgress 输入校验与挂载契约 |
| `G2-node-host` | `node_host.rs` | 3 / 0 / 0 | NodeHost 持 RunApplication 与默认 reader；端口接线预算 +70 行，列为架构欠债 |
| `G2-node-conversation` | `node_host/conversation.rs` | 1 / 0 / 0 | 随 G2 保留历史投影的 RunApplication 查询端口 |
| `G5-node-preparation` | `node_host/preparation.rs` | 0 / 4 / 0 | 随 G2/G8 保留 sandbox 工具与渠道包装装配；已删除工具名反向引用 |
| `G9-pilot-host` | `pilot_host.rs` | 0 / 3 / 0 | 保留 Pilot Turn 具体执行端口；端口接线预算 +35 行，列为架构欠债 |

G2（含 node_host 默认 reader）预算 +70 行、G8 +35 行、G9 +35 行的端口接线均未实施。G1/G3/G5/G10 随相关边界保留；A09–A16、B01–B07、C01 均明确属于这些命名项。任务书 H5 表逐条记录理由、关联与关闭条件，`layer-fence.txt` 逐行注释，测试固定 path/name 对应关系。

## 偏离与边界

- 按验收者裁决，实际剩余 20 / 14 / 1 不全部物理清除，而是固定为命名架构欠债；未登记项清零。此为已确认范围调整。
- G14 原盘点漏掉 `runtime_configuration::install_release_network` 对 `PluginLibrary::install_directory` 的生产调用。该路径为发行包内置 network Plugin 的启动安装；加 cfg(test) 会令生产构建失败，改锁入口又会超出本次保持语义的范围。因此保留原方法，不兑现原 -8 行预算。无需放宽行数约束，最终仍净减 5 行。没有为删除该入口改变安装并发语义。
- H3 的 Node 侧默认构造随 G2 保留；execution、durable cleanup 默认构造及配置副本已处理。
- HTTP 路由/JSON、磁盘路径/字节、权限、并发/恢复语义均未作为本项修改。Kernel diff 为空，测试既有行为断言保留，围栏上限未提高。
- 未读取 `.local/`，未修改主工作区，未部署、未推送。该结论不等于架构已验收；任务书状态交回“开发与规定检查完成，待架构验收”。
