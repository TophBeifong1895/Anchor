# H3 配置只在组合根读取交回

2026-10-25，基点 `2a9fdad`（H2 已验收），分支 `host/h3-root-configuration`，独立 worktree `/tmp/anchor-host-h3`。开发与规定检查完成，待架构验收；未合入 master、未推送、未部署。主工作区保持 master，原未跟踪路径未改动。

## 结果与替代关系

- `main.rs::HostConfiguration::from_env` 在进程入口各读一次 `ANCHOR_BWRAP`、`ANCHOR_RUNNER_LIBRARY_ROOT`、`ANCHOR_RUNNER_CATALOG_ROOT`、`ANCHOR_RUNNER_STATE_ROOT`，类型化配置传给 HTTP、独立执行、RunApplication、Pilot、Goose Pilot 与 Plugin 连接测试。替代下层直接读取、可从下层调用的根模块环境助手和 `ToolEnvironment::from_env`；这些旧读取路径已删除。
- Library 显式根优先于调用方已解析的 catalog 根；独立入口保留自己的 catalog 默认。工具环境仍只从显式 Library 根装载，不新增 catalog 回退。MCP OAuth 使用已经解析并授权的 Library 根，删除再次读取环境的 `resolve_oauth_library_root`。Run 存储仍比较注入的 state 根并做原来的规范化检查。
- 中立的 `model_fields.rs` 承接原 ModelFields 数据结构、默认值和 provider 身份算法，取代设置模块内的 DTO；序列化字段及顺序未变。删除 `ModelFields::values`，设置保存、凭据更新、连接测试与执行直接调用 Goose 的 `ModelRegistry::from_fields`。URL、凭据、模型名、协议、别名和正数限制沿用同一组校验；协议规则及原始数值解码也共用实现，入口保留原错误文本。
- 新增“保存上下文窗口 0 时拒绝且保留原文件字节”回归。核查发现旧保存路径原本也会在字符串转换后的 registry 校验中拒绝 0；本轮将类型化字段直接交给同一校验入口，不声称修复了原本可保存 0 的行为。
- 测试夹具改为注入配置，不再读写这四个全局变量。只为 Library 环境覆盖存在的 RAII 和锁已删除；仍读取其他部署字段的测试保留所需锁。原 OAuth 根回退测试移到组合根测试，断言保留。去掉 PreparedExecution 和测试助手已失去用途的 catalog/state 参数。
- 新增 Rust [配置围栏](../../../rust/anchor-runner-host/tests/configuration_fence.rs)，复用现有生产源码扫描器，要求四个完整变量名字面量各只在 main.rs 出现一次。扫描器默认行为不变，Run / 层次围栏继续忽略字面量。围栏是词法检查，不做常量求值或宏展开；动态 Plugin 环境字段另见清单。

## 测量

| 项目 | H2 基点 | H3 |
| --- | --- | --- |
| Host 服务 → 用例 | 25 | 25 |
| Host 服务 → 入口适配 | 23 | 23 |
| 用例 → 入口适配 | 1 | 1 |
| Run 围栏：生产文件 / 业务引用 / 运行时引用 | 175 / 0 / 56 | 176 / 0 / 56 |
| Host 生产物理行数 | 50,909 | 50,968（+59） |
| 目标变量：main.rs 之外的生产读取 | 多处 | 0 |

两份既有围栏上限文件未改动。[最终输出](fences.log) 无待降低提示。新增生产文件是承接已有 DTO 的 `model_fields.rs`；增加行数来自显式配置结构、参数传递与类型化构造，原重复读取和字符串转换已删除。没有为减少计数合并导入。

行数复用 `experiments/constitution-host-preflight/final/measure_lines.py::test_partition`，排除测试文件和 cfg(test) 区段，包含生产所属注释与空行；[line-counts.json](line-counts.json) 保存基点和仅含源码/围栏变更的 tree 身份。

按原测量工具的字面量保留方式核对整个 Host src 中既有 5,896 处断言：除格式、Library 访问器及测试配置构造函数替换外，断言条件与字面量保持一致；允许跨文件移动，新增的 0 窗口测试单独排除。[核对结果](assertion-check.json) 列出归一化项。

其余 `ANCHOR_*` 读取本轮不迁移；[清单](environment-inventory.md) 逐文件列出静态变量、常量/列表间接读取、Plugin 动态字段和易混入生产统计的测试读取。

## 实际验证

Cargo 命令均设置 `CARGO_TARGET_DIR=/home/mansteinl/Anchor/rust/target`；测试另设置 `XDG_RUNTIME_DIR=/run/user/1001`，临时数据位于 tmpfs。

| 命令或核对 | 实际结果 |
| --- | --- |
| `cargo +stable check --manifest-path rust/Cargo.toml -p anchor-runner-host --all-targets --locked` | 编译通过；当时提示一个新测试中未使用的扫描器入口，已修正，最终严格 clippy 无警告 |
| `scripts/test.sh -p anchor-runner-host --bin anchor-runner-host configuration --locked` | 95 passed / 0 failed；[输出](configuration-tests.log)。首轮 94 passed / 1 failed，旧测试要求固定读取顺序；恢复原顺序后通过，未改断言 |
| `scripts/test.sh -p anchor-runner-host --locked` | 最终 exit 0；1,231 passed / 0 failed / 110 ignored，41 个测试二进制；[汇总](test-results.json)、[完整输出](tests.log)。首轮全量也通过，合并剩余重复解析后对最终代码重跑 |
| `scripts/test.sh -p anchor-runner-host --test configuration_fence --test layer_fence --test run_access_fence --locked -- --nocapture` | exit 0；5 passed；配置围栏、25 / 23 / 1 与 Run 0 / 56 均通过；[输出](fences.log) |
| 配置围栏反向验证 | 在 tool_environment.rs 临时加入目标变量字面量，以已构建的配置围栏二进制执行，exit 101 并指出该文件；随后恢复原内容，最终围栏通过；[失败输出](configuration-fence-negative.log) |
| `cargo +stable clippy --manifest-path rust/Cargo.toml -p anchor-runner-host --all-features --all-targets --locked -- -D warnings` | exit 0；[输出](clippy.log) |
| `cargo +stable fmt --manifest-path rust/Cargo.toml --all -- --check` | exit 0 |
| `scripts/check-sensitive-wording.sh` | exit 0 |
| `git diff --check`、`git diff --cached --check` | exit 0 |
| `git diff 9a5fa57 -- rust/anchor-kernel` | 空 |
| 两份既有围栏上限文件相对 HEAD 的 diff | 空 |

## 未覆盖边界

110 个 ignored 未执行；未新增真实 Provider 端到端、Web 或部署验收。未读取 `.local/` 下的设置、凭据或运行状态；本机已保存设置是否通过共用校验仍由验收者核查。没有改变 HTTP 路由/JSON、磁盘路径与字节、权限或并发/恢复契约。其他部署环境读取保留在清单中，H4/H5 未开始。H3 无需要扩大范围的偏离。
