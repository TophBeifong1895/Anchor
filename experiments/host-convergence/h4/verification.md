# H4 Plugin 领域单一主人交回

2026-10-26，基点 `ac062db`（H3 已验收），分支 `host/h4-plugin-library`，独立 worktree `/tmp/anchor-host-h4`。开发与规定检查完成，待架构验收；未合入 master、未推送、未部署。

## 结果与替代关系

- `plugin_library::Catalog` 与 `PluginLibrary` 接管 Host 内分散的底层目录和库构造，内部对象不对外暴露，只包装实际使用的方法。替代模块外生产代码的 46 处 `FilePluginCatalog::new` 和 17 处 `Library::new`；模块外这两种构造均为 0。`InstalledPluginCatalog` 通过 `ReadCatalog` 持有原共享 guard，列表仍读取轻量元数据，选中详情才读完整定义。
- 原 `application/plugins.rs::plugin_publication_lease` 整体下沉，删除服务层对该用例函数的调用。20 ms 可取消重试、原错误类别与 Debug 文本保持；错误只在 application 边界转换一次，安装错误分类也复用该映射。普通观察不加锁，非阻塞共享读取、候选私有树和开发 scope 的生命周期保持，Graph → Library 及 catalog → Library 的顺序不变。准备租约仍在 OAuth/MCP 启动前释放。
- `Catalog::checked_definition` 统一读定义与重验；`ExpectedDefinition::matches` 供已捕获定义复用同一比较，不增加一次读取。授权等价、授权摘要、代码摘要、完整 binding 四种契约保持独立。候选探测仍先检查启用的 MCP server，再检查代码摘要；配置连接测试保留原读取失败、摘要变化和版本变化的错误顺序。持久配置字段、凭据或两次执行观察之间的比较仍归原事实所有者。
- `mount_declarations` 与 `merge_declarations` 替代 API、Node、Turn 三处声明归一化；只有配置的 Pilot Graph 可声明必需插件，默认值保持。声明直接交给既有 `plugin_mounts::resolve`，没有第二份 Session 开关合并实现。
- `plugin_library::tools` 承接原 `plugin_view` 的整组准备，新增普通连接步骤统一装配与披露归属挂接。OAuth 继续共用 `McpToolConfig::authorize_oauth`。Pilot 保持按顺序逐插件准备、软失败原因和取消清理，披露在 OAuth/连接失败前已插入；Node 保持整批准备与硬失败类别，在 OAuth 前保留配置，在连接前记录观察。删除旧装配的重复实现与已无生产调用的便利入口，没有回调或策略枚举。
- 新增 Rust [所有权围栏](../../../rust/anchor-runner-host/tests/plugin_library_fence.rs)，复用既有生产源码扫描器，禁止新模块外出现底层类型引用，连同 import/别名一起检测。它比只匹配构造调用更直接；不做名称解析、通配导入解析或宏展开。两份既有围栏没有提高任何上限。

## 测量

| 项目 | H3 基点 | H4 |
| --- | --- | --- |
| Host 服务 → 用例 | 25 | 22 |
| Host 服务 → 入口适配 | 23 | 23 |
| 用例 → 入口适配 | 1 | 1 |
| Run 围栏：生产文件 / 业务引用 / 运行时引用 | 176 / 0 / 56 | 178 / 0 / 56 |
| 模块外底层 Catalog / Library 生产构造 | 46 / 17 | 0 / 0 |
| 全 Host 底层 Catalog / Library 生产构造 | 46 / 17 | 1 / 2，均在 plugin_library.rs |
| Host 生产物理行数 | 50,968 | 51,256（+288） |

层次下降来自 `node_host/preparation.rs`、`node_host/turn_plugins.rs`、`plugin_view.rs` 对用例发布租约的引用删除，逐文件上限同步降低；Run 上限文件未变。[围栏输出](fences.log) 无待降低提示，[构造盘点](constructors.log) 使用同一词法扫描器排除独立与内联测试。

行数复用 `measure_lines.py::test_partition`，包括生产所属注释与空行；[line-counts.json](line-counts.json) 记录基点和源码 tree。增加主要来自拥有私有底层对象的实际使用接口、独立错误映射与显式共享步骤；原构造调用、重验及归一化副本已替换，不以行数下降作为完成条件。

核对整个 Host src 的 5,898 处既有断言，保留字面量、比较宏平衡正文，允许构造名与已移动准备模块路径归一化，忽略格式和尾逗号后，多重集合完全一致，无删除或增加。[核对结果](assertion-check.json)。新增的两项所有权围栏测试位于独立 tests 文件。

## 实际验证

Cargo 命令均设置 `CARGO_TARGET_DIR=/home/mansteinl/Anchor/rust/target`；测试另设置 `XDG_RUNTIME_DIR=/run/user/1001`，临时数据位于 tmpfs。

| 命令或核对 | 实际结果 |
| --- | --- |
| `cargo +stable check --manifest-path rust/Cargo.toml -p anchor-runner-host --all-targets --locked` | 迁移期间执行；发现并修正遗漏的测试导入和未用入口，最终代码由下列全量与严格 clippy 编译覆盖 |
| `scripts/test.sh -p anchor-runner-host --bin anchor-runner-host plugin --locked` | 169 passed / 0 failed；[输出](plugin-tests.log) |
| `scripts/test.sh -p anchor-runner-host --locked` | exit 0；1,233 passed / 0 failed / 110 ignored，42 个测试二进制；[汇总](test-results.json)、[完整输出](tests.log) |
| `scripts/test.sh -p anchor-runner-host --test plugin_library_fence --test layer_fence --test run_access_fence --test configuration_fence --locked -- --nocapture` | exit 0；7 passed；所有权 0 / 0、层次 22 / 23 / 1、Run 0 / 56 与配置围栏通过；[输出](fences.log) |
| 所有权围栏反向验证 | 在 runtime_configuration.rs 临时引入底层 Catalog 别名，以已构建围栏二进制执行，exit 101 并指出该路径；恢复原字节后最终围栏通过；[失败输出](plugin-library-fence-negative.log) |
| `cargo +stable clippy --manifest-path rust/Cargo.toml -p anchor-runner-host --all-features --all-targets --locked -- -D warnings` | exit 0；[输出](clippy.log) |
| `cargo +stable fmt --manifest-path rust/Cargo.toml --all -- --check` | exit 0 |
| `scripts/check-sensitive-wording.sh` | exit 0 |
| `git diff --check`、`git diff --cached --check` | exit 0（源码交回） |
| `git diff 9a5fa57 -- rust/anchor-kernel` | 空 |

## 接口简化与未覆盖边界

确认后的设计要求只包装实际使用方法、沿用现有错误和普通共享步骤。实现没有加入草案中的 `Publication` 聚合对象、`LibraryError` 或只转发的 OAuth 函数：发布申请集中在服务内，返回原 `InstallLease`，保持调用方已经验证的持有范围；`ReadCatalog` 仅聚合原列表读取的共享 guard。`PublicationError` 只承接原用例的两个错误类别，以保持取消、工作线程和安装失败的原文本；底层 Graph/Install/Development 错误仍直接沿用。这些是可逆接口简化，没有改变锁或产品语义。

110 个 ignored 未执行；没有新增真实 Provider 端到端、Web 或部署验收。未读取 `.local/` 下的设置、凭据或运行状态。HTTP 路由/JSON、磁盘路径与字节、权限和并发/恢复契约没有设计变化，Kernel diff 为空。H3 关于 RunApplication 配置副本与默认配置构造的遗留仍按任务书归 H5，本阶段未扩边界。
