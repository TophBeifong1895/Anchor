# H1 共享原语与已分叉规则交回

2026-10-25，基点 `6ca72054bd28f99289cf1a6334946a4baba3492c`，分支 `host/h1-shared-primitives`。H0 已验收并在 master；本项开发与规定检查完成，待架构负责人验收，尚未合入 master、未推送、未部署。

## 结果与替代关系

- [anchor-runtime::identifier](../../../rust/anchor-runtime/src/identifier.rs) 是 Plugin id 与名称字符规则的唯一实现。删除 Host `pilot_tools::identifier`、API 的 Plugin id helper、local_inputs 的两个 helper、Graph 管理和渠道管理的名称 helper、Run 数据的 safe_component 及 RunApplication::valid_run_component；保留只负责原错误映射/长度上限的领域函数。Library 安装/回收与 Graph authoring/catalog 的字符判断改为调用同一原语。
- [host_fs](../../../rust/anchor-runner-host/src/host_fs.rs) 取代 `goose_acp::pilot::directory` 及分散的 invocation-key SHA-256 主干实现。目录函数除导入位置外函数体逐字核对一致，原错误文本保留；组合根的 `create_durable_directory` 保留其跟随符号链接的语义，并加注释说明差别。
- 七处主干实现收口：artifacts、run_data、assistant、GooseNodePort、node_host::facts、run_access::fixtures 和 Goose 集成夹具。前三处直接导入共享函数，Goose 的关联 helper 已删；`nf1-`、`fixture-`、`np1-`、`fs1-`、`fs2-` 等前缀及后缀、序列化和路径拼接保持原值。集成夹具直接编译同一 Host 原语模块。其它仅为测试准备独立期望字节的 fixture 哈希未改写。
- 凭据、配置、Plugin 工具及渠道 Plugin 绑定不再把 `-x`、`_x` 当合法 Plugin id。名称调用保持原来允许 `-`、`_`、`.` 开头（但排除单独 `.`/`..`）的行为。逐调用点分界见 [call-sites.md](call-sites.md)，同表列入提交说明。

原本并非相同规则的代码没有在本项强行归一：包/环境变量/模型槽名称有自己的字母表；某些旧路径的预检仅检查字符、不排除 `.`/`..`，后续再执行路径核查。这些保留原行为，未套用更严的新名称校验。local input grant 名和旧 tool id 的规则原本与 Plugin id 相同，复用严格原语而不放宽为一般名称。

## 测量

| 项目 | 前 | 后 |
| --- | --- | --- |
| Host 服务 → 用例 | 58 | 58 |
| Host 服务 → 入口适配 | 35 | 23 |
| 用例 → 入口适配 | 1 | 1 |
| Run 围栏：生产文件 / 业务引用 / 运行时引用 | 174 / 0 / 56 | 175 / 0 / 56 |
| Host 生产物理行数 | 50,958 | 50,878（−80） |
| Runtime 生产物理行数 | 6,032 | 6,034（+2） |
| Graph Host 生产物理行数 | 3,416 | 3,410（−6） |
| Library 生产物理行数 | 4,216 | 4,198（−18） |

四个 crate 合计减少 102 行生产物理行。行数复用既有 `measure_lines.py` 的测试分区方法，包含所属生产区的注释/空行，不是 SLOC；源码树身份与结果见 [line-counts.json](line-counts.json)。新增四项测试为 Runtime 规则边界、凭据入口、配置入口及固定 invocation 哈希字节；原有测试只修改模块引用/构造路径，没有改变断言。

`layer-fence.txt` 删除六个已归零文件的旧上限，合计下降 12：credential_store（6）、credential_store/management（1）、pilot_interaction/initialization（1）、pilot_attachments（2）、plugin_configuration（1）、plugin_configuration_test（1）。未提高任何上限，也未改 Run 围栏基线。

## 实际验证

| 命令 | 结果 |
| --- | --- |
| `XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runner-host --bin anchor-runner-host plugin_ids_require_an_alphanumeric_prefix --locked -- --nocapture`（只加测试，修复前） | 预期 exit 101；2 failed，均因入口放过错误前缀、后续才报目标不可用；[原始输出](red.log) |
| 同上（修复后） | exit 0；2 passed；[原始输出](green.log) |
| `XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runtime --lib identifier --locked -- --nocapture` | exit 0；1 passed |
| `XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runner-host --bin anchor-runner-host invocation_filenames_preserve --locked -- --nocapture` | exit 0；1 passed，固定哈希字节一致 |
| `XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runner-host --test layer_fence --test run_access_fence --locked -- --nocapture` | exit 0；4 passed；层次 58 / 23 / 1，Run 0 / 56 |
| `XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runner-host -p anchor-runtime -p anchor-graph-host -p anchor-library --locked` | exit 0；1,525 passed / 0 failed / 110 ignored；[分 crate 摘要](test-results.json) |
| `cargo +stable clippy --manifest-path rust/Cargo.toml -p anchor-runner-host -p anchor-runtime -p anchor-graph-host -p anchor-library --all-features --all-targets --locked -- -D warnings` | exit 0 |
| `cargo +stable fmt --manifest-path rust/Cargo.toml --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |
| `scripts/check-sensitive-wording.sh` | exit 0 |
| `git diff 9a5fa57 -- rust/anchor-kernel` | 空 |
| `git diff HEAD -- rust/anchor-runner-host/run-access-fence.txt` | 空 |

四 crate 全量中 Host 1,229 / 0 / 110、Runtime 157 / 0 / 0、Graph Host 65 / 0 / 0、Library 74 / 0 / 0，包含文档测试。完整输出仅在本机 `/tmp/anchor-host-h1-tests.log`、`/tmp/anchor-host-h1-clippy.log`；110 个 ignored 未执行，未把编译它们当作 Goose 或真实 Provider 验收。全量后没有源码变更，因此未重复运行同一全量。

## 边界

未读取 `.local/`；本机已安装 Plugin id 是否合规仍由验收者核查。没有更改合法对象的 HTTP 路由/JSON、持久化格式或字节、权限、并发/恢复语义；错误前缀 Plugin id 的入口校验收紧是任务书决定 3 的已定行为。未新增真实 Provider、ignored Goose、Web 或部署验收。

除把搜索发现的同规则副本（Library 回收、Run/Artifact 名称等）一并收口外，没有扩展阶段范围；这些是 H1「各留一份」的直接要求。H2 尚未开始。
