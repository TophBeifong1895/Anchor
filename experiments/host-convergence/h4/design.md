# H4 Plugin 领域单一主人：只读设计交回

状态：设计草案，待任务书要求的验收确认；没有实施 H4，仅新增设计产物与任务状态，没有修改生产源码、行为、权限或持久事实。盘点时间 2026-10-25；任务实际基点为 master `2152fd9`；它在 `e3e8384` 之后只增加文档提交，盘点涉及的生产代码相同。独立分支 `host/h4-plugin-design`，没有读取 `.local/`，没有部署或推送。H1–H3 实施期间行号会移动，以下以文件、函数和当前行号共同定位。

## 1. 实测范围与当前依赖

当前 `anchor-runner-host/src` 生产构造为 **FilePluginCatalog::new 46 处 / 26 个文件，Library::new 17 处 / 11 个文件，共涉及 29 个文件**。原任务书的 72 / 33 不能直接作为生产代码验收基线。简单排除路径名含 `test` 后仍会保留内联测试，同时错误漏掉生产模块 `plugin_configuration_test.rs`。本盘点用 H0 的 `tests/common/mod.rs::production_counts` 词法器复核：扫描 174 个生产文件，排除注释/字面量、独立测试及 `#[cfg(test)]` 项，结果仍是 46 / 17；保留了这个连接探测生产模块的 2 处构造。下文逐项列出全部 63 处，因此可重新核对数量。`bin/anchor-net-relay.rs` 没有这两种构造。

依据为 `docs/architecture.md:5` 的入口→应用→Runner→Node Runtime 链、`:218` 的 Graph lease→Library publication lease、`docs/plugins.md:45` 的 Pilot/Node 共用披露、`:51` 的共享非阻塞读取、`docs/plugin-dependencies.md:90` 的当前 Library 代码与 `:123` 的唯一库根。没有遍历历史文档。

当前组合与依赖：

- `main.rs:1` 把 application、node_host、pilot_tools、tool_host、plugin_view 等都放在二进制根；`:251` 的 `library_root` 仍读环境，是 H3 对象。H4 接收 H3 已解析根，不自行重新读环境，也不从 bundle 推导库根。
- `application.rs:14`–`:24` 分出开发环境、候选、Plugin 与 Run 门面。`application/plugins/catalog.rs:50` 的 InstalledPluginCatalog 已拥有“列表/选中详情的共享 guard 生命周期”，但直接构造底层库和目录。
- `node_host.rs:93` 的 HostIoResolver 持 Library 根、fixture、MCP 配置、Session mount source 与 Pilot Graph；`:100` 仍持 RunApplication。后者的替换归 H5，H4 不扩大。
- `node_host/preparation.rs:41` 与 `pilot_tools.rs:125` 分别组装执行工具；前者批量硬失败，后者逐 Plugin 软失败。两者都反向调用 `application::plugins::plugin_publication_lease`；真正执行准备还通过 `plugin_view.rs:503` 再调用该函数。
- `tool_host.rs:152`/`:191` 属于 MCP 配置与连接适配。`:197` 构造 catalog 仅对已读定义做 MCP 展开，没有再次读取清单；应保留这个区别。
- `plugin_mounts.rs:331` 已经有唯一的 Session switch/default 合并算法。重复的是 API / Node / Turn 各自把原始 PluginRef 变成 `required && pilot` 的声明；不能在新模块另写第二份合并算法。

## 2. 租约词汇与不得改变的边界

- **R**：`Library::catalog_read_guard`，`anchor-library/src/lib.rs:109`。对 `plugins` 目录取非阻塞共享 flock；目录缺失返回 `None`，不创建目录；忙直接失败。不应改成等待锁或独占锁。
- **P**：`InstallLease`，同文件 `:32`/`:163`；`application/plugins.rs:505` 当前通过 `try_install_lease` + 20 ms 可取消重试等待独占发布锁。它还保护依赖落地/回收、引用发布与挂载选择；不是“所有读取统一取共享锁”。
- **D**：`DevelopmentScope` 的 shared scope lease；register 还在依赖 writer lock 内。不能替换为 Plugin P。
- **C**：候选私有临时树/准备目录，候选 `preparation.lock` 为非阻塞独占；不是正式库，不另取正式 Plugin 发布锁。
- **O**：当前调用点没有自己的 Library 锁，仅现场观察，或依靠更外层不同锁。下表诚实标 O，不宣称已有原子快照。H4 先保留这些时序；若要给 O 增加 R/P，会改变并发/失败行为，须作为单独提案确认。

当前锁顺序保留：准入 graph→Library→active；挂载/卸载 catalog→Library。不得在持 graph 时再取 catalog；不得在已有 P 下再次取 R/P。选择和定义/视图准备各自的 P 作用域结束后，才进入 OAuth/MCP；正式安装的 P 则从依赖落地一直保护到发布及失败清理结束。没有把共享 guard 持有到模型执行的设计。

## 3. 全量 FilePluginCatalog 构造清单（46）

下表路径均相对 `rust/anchor-runner-host/src/`。同一格有多个行号时，每个行号都是独立构造，分别注明用途。

| # | 位置 | 用途 | 当前租约与迁移方式 |
| --- | --- | --- | --- |
| 1 | `api.rs:908` session_plugin_mount_target | 开启挂载前检查已安装定义 | O 的只读预检；真正写入由调用方 catalog/P 下重验。迁移 observe_definition。 |
| 2 | `api/oauth.rs:306` resolve_binding_at_root | 从 MCP 声明派生 OAuth route、授权摘要 | O；不在 OAuth 网络等待期间持库锁。迁移 observation catalog。 |
| 3 | `api/plugins.rs:127` plugin_file | 读定义及选中文件 | O；现有 `open_resource` 保持。不得顺手扩大到新的读取锁/披露分类规则。 |
| 4 | `application/channel_management.rs:111` channel_plan | 发现 assistant work node 的 channel Plugin | O；仅读渠道声明，调用场景存在各自 Graph/catalog 锁。 |
| 5 | `application/graphs.rs:110` inspect_graph_definition | 验证 Graph 引用 id 是否可解析 | O 的校验，不写执行绑定。 |
| 6 | `application/graphs.rs:256` referenced_plugin_bindings | 根据 bundle id 引用读当前 binding map | O helper；调用方的准入/发布时序原样保留。 |
| 7 | `application/graphs.rs:269` resolve_graph_definition | Graph 创建/保存时返回 catalog 与 bindings | 写路径 `:364` 有 R；其它调用路径不自行有锁，不能把函数内部改成嵌套锁。返回 Host Catalog adapter，避免泄露具体构造。 |
| 8 | `application/plugin_candidates.rs:266` prepare_plugin_candidate | 已 stage 候选读 binding 并重验 review digest | C，持候选 preparation.lock。 |
| 9 | `application/plugin_candidates.rs:360` test_plugin_candidate | 候选准备后重验代码并读 MCP，供候选自测 | C，PreparedCandidate 保留准备锁和 dependency scope；不拿正式库 OAuth 授权。 |
| 10 | `application/plugins/catalog.rs:71` inspect | 列表 metadata 与依赖就绪投影 | R 随 InstalledPluginCatalog 活到外层投影结束；元数据轻读取，不能换 full definition 扫正文。 |
| 11 | `application/plugins/catalog.rs:122` definition | 选中详情的 full definition | 同一个 R；缺库时现有空列表/选中错误语义保持。 |
| 12 | `application/plugins.rs:127` plugin_install_precondition_with_access | 私有 stage 的完整定义，提取授权/联网/mutable | C（TempDir）。 |
| 13 | `application/plugins.rs:133` 同函数 | 预检已安装 existing_digest | O，用户确认前观察；发布前还会 P 下重验。 |
| 14 | `application/plugins.rs:347` install_plugin_if_unchanged | 先复查 installed digest | catalog mutation guard；这里还没取 P。保留两次重验的时点。 |
| 15 | `application/plugins.rs:388` 同函数 | 复制批准候选后再核对 installed digest | P，覆盖后续依赖落地/发布；同一事务。 |
| 16 | `application/plugins.rs:433` 同函数 already_installed 分支 | 仅修复视图后构造 InstallOutcome，不覆盖 Plugin 字节 | 现有 P 变量作用域与 catalog guard 保留。 |
| 17 | `application/plugins.rs:544` remove_unreferenced_view | 失败清理前判断已安装 Plugin 是否仍引用 view | 明确传入 `&InstallLease`，P；不明现场保留 view。 |
| 18 | `application/plugins.rs:679` plugin_uninstall_precondition | 卸载初始定义及 mutable 大小预检 | O 的预检；最终删除另有 catalog/P 再验，不新增预检授权。 |
| 19 | `application/plugins.rs:710` 同函数 network 分支 | 扫描依赖 network 的插件，汇总会话/Run/Graph 引用 | O 的影响投影，不成为执行准入事实。 |
| 20 | `application/run_access/kernel_calls.rs:186` KernelCallHost::call | 给 InProcessGraphHost 提供子调用 PluginCatalog | 外层 Host 的 admission/reference lease 契约；构造本身 O。需实现 PluginCatalog 的 Host Catalog adapter，不能从 run_access 偷拿第二把锁。 |
| 21 | `assistant_plugins.rs:96` publish | 操作员保存 Graph 时冻结 Plugin authorization map | `:93` R，调用前已持 Graph lease。 |
| 22 | `channel_supervisor.rs:66` ChannelResourceGuard::valid | 监听进程存活检查当前渠道声明 | O 的反复现场观察，不能改成一次冻结或长持锁。 |
| 23 | `channel_supervisor.rs:257` discover | 从 Graph id 找渠道 entrypoint/声明 | O，维持 bundle 校验与可执行普通文件检查。 |
| 24 | `channel_supervisor/management.rs:290` definition | 管理写路径读取当前完整定义 | O helper，外层管理锁维持，不自行宣称 P。 |
| 25 | `channel_supervisor/management.rs:300` metadata | 轻量渠道管理投影 | O helper；不能换成 full definition。 |
| 26 | `credential_store/management.rs:8` definition | 管理凭据入口的 Plugin 校验与声明读取 | O helper；configure 文档要求 API 外层 catalog/reference lease，凭据自身 lease 是另一把锁。 |
| 27 | `credential_store.rs:368` request | 原生凭据表单目标/授权检查 | 读取定义在 credentials lease 之前；保留顺序。 |
| 28 | `credential_store.rs:514` submit | 收表单后重读当前授权/配置 | 已持 credentials lease；不要在这里突然新增等待 P 造成新锁顺序。 |
| 29 | `goose_acp/pilot_interaction/initialization.rs:35` policy | 原生授权请求前的 Plugin binding 观察 | O，等待人确认时不持 Library 锁。 |
| 30 | 同文件 `:79` | 原生确认后精确 binding 重验 | 先 catalog mutation guard，再 application plugin_reference_lease（P）；随后保存 Session 政策。 |
| 31 | `node_host/mounts.rs:97` session_plugin_mounts | 无 Graph 的 Pilot Session switches→当前定义/binding/view | 调用者持 selection P；fixture 无需库字节。 |
| 32 | 同文件 `:392` mounted_plugins_unlocked | 普通 Run 声明+Session switches 求挂载并验授权 | 同步 wrapper `:334` 用 R；异步 prepare 的调用者用 selection P。需继续区分两条入口，不能内部再加锁。 |
| 33 | `node_host/preparation.rs:25` validate_plugin_configuration | 准入时验证当前授权，与运行期私有配置区分 | 注释说明调用方持准入 publication lease；当前 helper 不自己取锁。 |
| 34 | `node_host/resolution.rs:29` resolve_plugins | fixture 优先，再读真实当前 Library binding | O helper，Turn/准入调用方按原协议持锁；不新增 bundle fallback。 |
| 35 | `node_host/turn_plugins.rs:139` freeze_assistant_plugins | 工作节点 Graph 声明+Session→冻结 Turn 授权 | 外层 graph→P；Turn 重试重用既有快照。 |
| 36 | `plugin_configuration.rs:66` definition | owner 配置读取/写入/版本检查的定义 helper | O；credentials lease 在其它函数管理，不能与 Library 锁混为一谈。 |
| 37 | `plugin_configuration_test.rs:32` test | 连接探测前核对授权摘要与 revision | O 的预检，后续 invocation preparation 会 P 下再验。这个文件是生产代码。 |
| 38 | 同文件 `:115` | 探测后核对 current、配置身份及 revision | O 的结果时效投影；不把“探测成功”当真实业务成功。 |
| 39 | `plugin_disclosure.rs:462` read | 执行披露前重验授权和当前公开文件分类 | `:458` R，必须覆盖分类、安全 open 和整个读取；正文热更新允许，授权变化要求新 invocation。 |
| 40 | `plugin_network.rs:38` resolve | 联网需求、network provider 与 view 就绪求值 | 调用方可能持 selection P/R，函数自身 O；不要变成内部嵌套 guard。 |
| 41 | 同文件 `:142` apply | 需要时追加 network 的 binding | 跟随 resolve 的同一调用方锁，维持过滤顺序与 reasons。 |
| 42 | `plugin_view.rs:197` prepare_for_plugin | 读单 Plugin 定义并检查已安装 view | O helper；调用来源有管理预检、selection P 和修复路径，调用契约分别保留。 |
| 43 | 同文件 `:537` prepare_plugins_for_owner_async | 当前定义/授权重验，派生 views、mounts、披露与 MCP | P 在 spawn_blocking 中覆盖所有文件读取，返回之前释放，OAuth/MCP 在外。 |
| 44 | `runtime_configuration.rs:123` install_release_network | 发行内 network bootstrap 的已安装检查 | 初始观察 O；安装器自身取 P。错误安装目录不覆盖。 |
| 45 | `tool_host.rs:157` McpToolConfig::from_catalog | 候选/已有调用的 MCP 初步 wiring | O 或调用方 C/P；只能重用原调用方时序。候选自测是生产调用，不能因多数调用来自测试就删除。 |
| 46 | 同文件 `:197` from_definitions_for_owner_execution | 对已读定义展开 MCP 模板/owner 环境 | 没有重读 manifest；执行调用在 prepare P 下。移走构造，保留 from_definitions 对照同一份定义。 |

## 4. 全量 Library 构造清单（17）

| # | 位置 | 用途 | 当前租约 |
| --- | --- | --- | --- |
| 1 | `application/development_dependencies.rs:116` | select_development_view 的 scope 获取 | D，`development_scope(scope,false)`，返回持有期覆盖使用 |
| 2 | 同文件 `:182` | release_development_scope | Library 内独占 scope/tombstone 协议；不是 Plugin P |
| 3 | `application/development_environment.rs:176` | 现有或新建私有开发 scope | D；register 在 dependency write_lock 内，交还 LeasedEnvironment |
| 4 | `application/graphs.rs:364` | 保存 Graph 前 R | R 覆盖引用校验和 bundle 写入 |
| 5 | `application/plugin_candidates.rs:245` | stage 私有候选 | C 的临时树，无正式库锁需求 |
| 6 | 同文件 `:299` | 候选依赖使用的 development_scope | D，由 PreparedCandidate 持有 |
| 7 | `application/plugins/catalog.rs:58` | InstalledPluginCatalog::open | R，缺 plugins 目录返回 None→空目录 |
| 8 | `application/plugins.rs:124` | 安装预检 stage_directory | C，临时树 |
| 9 | 同文件 `:337` | 先 review_directory 重验待批准源 | catalog mutation guard；这里尚无 P |
| 10 | 同文件 `:384` | P 下复制/核对批准候选字节 | P + 私有 transaction |
| 11 | 同文件 `:463` | install_directory_locked | 已有 P，保护发布和失败 view 清理 |
| 12 | 同文件 `:515` | try_install_lease 循环 | P 的唯一可取消申请实现，应整体下沉并保持 20 ms 策略 |
| 13 | `assistant_plugins.rs:93` | publish 前 R | Graph→R |
| 14 | `node_host/mounts.rs:334` | 同步 Op.run 挂载求值 R | 非阻塞 R；异步 Agent 工具路径不用这层 |
| 15 | `pilot_tools/plugins.rs:156` | review_directory 的人类可读候选审查 | C，不发布、不改库 |
| 16 | `plugin_disclosure.rs:458` | 执行披露全读取 R | R 覆盖定义/公开路径交集/打开/读取 |
| 17 | `runtime_configuration.rs:132` | bundled network review + install | review 无发布锁，install_directory 内部自己取 P |

最终目标为 Host 生产代码的这两种 `::new` 都只出现在新服务内部。上表没有必须在 Host 新模块之外保留的生产构造例外。`anchor-library`、`anchor-graph-host` 自身及其它 crate 的构造不在 Host H4 的验收范围；它们不能依赖 Host，不能为清零搜索而倒置依赖。测试 fixture 仍可直接构造。

## 5. plugin_library 接口（签名级草案）

新模块是已有构造、生命周期与声明归一化的搬迁，不新增状态文件、缓存、后台任务或 Plugin 历史。接收 H3 的类型化根；这些接口都不读环境。错误独立于 ApplicationError / HTTP，外层保留现有错误文本和类别映射。

```rust
pub(crate) struct PluginLibrary { root: PathBuf }
pub(crate) struct Catalog { inner: FilePluginCatalog } // private inner
pub(crate) struct ReadCatalog {
    catalog: Catalog,
    guard: Option<anchor_file_lease::FileLease>,
}
pub(crate) struct Publication {
    library: Library,
    catalog: Catalog,
    lease: anchor_library::InstallLease,
}
pub(crate) enum LibraryError {
    Catalog(GraphError), Install(InstallError), Cancelled(String), Task(String),
}

impl PluginLibrary {
    pub(crate) fn new(root: PathBuf) -> Self;
    pub(crate) fn root(&self) -> &Path;
    // Preserve existing O observations. No implicit lock or new failure branch.
    pub(crate) fn observe(&self) -> Catalog;
    pub(crate) fn read(&self) -> Result<ReadCatalog, LibraryError>;
    pub(crate) async fn publication(
        &self, access: &DependencyAccess, operation: &str,
    ) -> Result<Publication, LibraryError>;
    pub(crate) fn review_directory(
        &self, id: &str, source: &Path,
    ) -> Result<PluginReview, LibraryError>;
    pub(crate) fn stage_directory(
        &self, id: &str, source: &Path, transaction: &Path,
    ) -> Result<(PathBuf, PluginReview), LibraryError>;
    // Explicit candidate root: never an execution fallback for installed Plugins.
    pub(crate) fn candidate_catalog(staged_root: &Path) -> Catalog;
    pub(crate) fn development_scope(
        &self, scope: &Path, create: bool,
    ) -> Result<Option<DevelopmentScope>, DevelopmentError>;
    pub(crate) fn release_development_scope(
        &self, scope: &Path,
    ) -> Result<(), DevelopmentError>;
    // Bootstrap preserves the installer's own lease path.
    pub(crate) fn install_directory(
        &self, id: &str, source: &Path, replace: bool,
    ) -> Result<InstallOutcome, LibraryError>;
}
impl ReadCatalog {
    pub(crate) fn catalog(&self) -> &Catalog;
    pub(crate) fn exists(&self) -> bool; // guard.is_some(); missing root stays empty
}
impl Publication {
    pub(crate) fn catalog(&self) -> &Catalog;
    pub(crate) fn install_directory(
        &self, id: &str, source: &Path, replace: bool,
    ) -> Result<InstallOutcome, LibraryError>; // calls install_directory_locked
    pub(crate) fn lease(&self) -> &anchor_library::InstallLease;
}
impl Catalog {
    pub(crate) fn definition(&self, id: &str) -> Result<PluginDefinition, LibraryError>;
    pub(crate) fn metadata(&self, id: &str) -> Result<PluginMetadata, LibraryError>;
    pub(crate) fn channels(&self, id: &str) -> Result<Vec<ChannelDefinition>, LibraryError>;
    pub(crate) fn plugin_directory(&self, id: &str) -> Result<PathBuf, LibraryError>;
    pub(crate) fn mcp_servers(
        &self, id: &str, resolve_env: bool,
    ) -> Result<Vec<McpServerDefinition>, LibraryError>;
    pub(crate) fn resolve_mcp_servers_with(
        &self, definition: &PluginDefinition,
        environment: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Vec<McpServerDefinition>, LibraryError>;
    pub(crate) fn checked_definition(
        &self, id: &str, expected: ExpectedDefinition<'_>,
    ) -> Result<PluginDefinition, DefinitionError>;
}
impl PluginCatalog for Catalog {
    fn resolve(&self, ids: &[String]) -> Result<Vec<PluginBinding>, GraphError>;
}
```

`development_scope` 与 `release_development_scope` 签名已对照 `anchor-library/src/development.rs:221`/`:268`；前者返回 `Result<Option<DevelopmentScope>, DevelopmentError>`，后者返回 `Result<(), DevelopmentError>`；两者保留 Library 原错误类型。scope 的 `register(&self) -> Result<(), DevelopmentError>` 和 `root(&self) -> &Path` 也直接沿用原类型，见同文件 `:343`/`:351`，不增加新的回收行为。`Publication` 逐步取代上层裸 InstallLease 的构造；若 Runner 端口必须返回 Box<dyn RunLease>，小包装可直接持有 Publication。已有函数需要借用 InstallLease 的场合（view 清理、Library API）继续借用，不能再创建。不要暴露 `Library` 或 `FilePluginCatalog` 本体给消费者来旁路生命周期。

`InstalledPluginCatalog` 留在 application/plugins/catalog.rs：entries 排序、unavailable 状态、面向适配器的 ApplicationError/JSON 投影仍是列表用例。只把 root 和 guard 改为持有 ReadCatalog；inspect/definition 通过同一个 Catalog，不新加一次磁盘读取。

### 摘要重验

```rust
pub(crate) enum ExpectedDefinition<'a> {
    Authorization(&'a PluginBinding),
    AuthorizationDigest(&'a str),
    CodeDigest(&'a str),
    ExactBinding(&'a PluginBinding),
}
pub(crate) enum DefinitionError {
    Read(LibraryError), Mismatch,
}
```

所有“读定义再比摘要/授权”的共同算法只在 `Catalog::checked_definition`：只读一次 full definition；按调用方的原契约比较；返回这同一份定义供 mounts/view/MCP/disclosure 使用。不要把四种比较合并成一个更严格的新检查：

- invocation/admission 继续 `PluginBinding::same_authorization`（runtime `graph/ports.rs:279`，比较 id/authorization_digest/mcp_servers）；允许代码正文更新。
- 披露继续仅核对 authorization_digest，然后在同一个 R 下按公开路径交集读当前正文。
- 候选和批准内容核对 code digest，不能用授权摘要替代代码检查。
- 原生 mount 确认后目前比较完整 binding，仍完整比较，不能放宽成 same_authorization。

原错误原因由调用者映射 `Mismatch`，保持已有字符串、GraphError::CorruptRun / Unsupported / ResolutionError 分类。解析/摘要计算仍归 FilePluginCatalog，Host 不复制哈希算法。

## 6. 挂载声明单一来源

```rust
pub(crate) struct MountDeclaration<'a> {
    pub(crate) id: &'a str,
    pub(crate) required: bool,
    pub(crate) default_on: bool,
}
pub(crate) struct DeclarationProjection {
    pub(crate) required: bool,
    pub(crate) mounts_by_default: bool,
}
pub(crate) fn mount_declarations<'a>(
    references: &'a [PluginRef],
    declaring_graph: Option<&str>, configured_pilot_graph: Option<&str>,
) -> Vec<MountDeclaration<'a>>;
pub(crate) fn merge_declarations(
    target: &mut BTreeMap<String, DeclarationProjection>,
    declarations: &[MountDeclaration<'_>],
);
```

唯一归一化：declaring_graph 必须 Some 且等于 configured_pilot_graph，才承认 required；default_on = required || reference.default.unwrap_or(true)。`merge_declarations` 对同一 id 的 required 和 mounts_by_default 分别 OR，复用 API 对多个 Run/node 的现有聚合结果。

新模块仅负责求声明及声明投影。现有 `plugin_mounts::resolve` 已是唯一的开关合并算法，保持它的实现、位置和签名，由 Node/Turn 调用方直接调用；不新增求值包装接口。调用方只把 MountDeclaration 的字段机械转换为现有 `(&str, bool, Option<bool>)` 输入：`(id, required, Some(default_on))`，不再次判断 Pilot 身份或默认值。这样 snapshot 的 switches 胜出、required 忽略 off、未声明的显式 on 可挂、缺失 Plugin 的原因和 by_declaration / by_switch 排序全部仍由原函数负责。`plugin_mounts` 的 Session source、MountSnapshot 持久化及冻结实现也保持原位。

API 的 session_plugin_declarations 提供从 run_access 读取到的原始 Graph name + PluginRef；node_host/mounts 和 turn_plugins 各提供自己当前已验证的快照。服务不读取 Run，不构造 RunApplication。普通 Run 当前针对“整张图存在不可信 required”的原因提示保留原作用范围；不要改成仅当前 node 才提示。Turn 冻结、重试、Session 审计事件保持原样。

## 7. Pilot/Node 共用准备与连接管线

建议实现放 `plugin_library/tools.rs`，不是新的通用执行框架。移动 `plugin_view::prepare_plugins_for_owner_async` 到此处；plugin_view 继续只负责从已读定义生成 view/挂载，tool_host 继续负责 MCP transport/fixture/沙箱连接。工具包装（NodeTools、channel_tools、conversation_tools、ToolDisclosure）留在原调用方，防止新 Host 服务反向依赖入口适配。

以下三回调签名是**待验收的候选形状**，用于暴露现有时序约束，并非已定实施接口。H3 合入后按届时已有类型再精简；只抽两条调用路径真实共用的步骤，不构建通用管线或 Hook 扩展系统。

```rust
pub(crate) enum ToolFailurePolicy {
    PerPluginSoft, // Pilot: ordered one-Plugin groups
    BatchHard,     // Node: one complete batch
}
pub(crate) struct PluginToolRequest<'a> {
    pub(crate) library: Option<&'a PluginLibrary>,
    pub(crate) bindings: Vec<PluginBinding>,
    pub(crate) fixture: &'a PluginToolHost,
    pub(crate) credentials: Option<(&'a Path, &'a str)>,
    pub(crate) oauth_owner: Option<String>,
    pub(crate) environment: ToolEnvironment,
    pub(crate) network: bool,
    pub(crate) cancellation: Cancellation,
}
pub(crate) struct PreparedPluginTools {
    pub(crate) bindings: Vec<PluginBinding>,
    pub(crate) ports: Vec<Arc<CombinedPluginTools>>,
    pub(crate) disclosure: PluginDisclosureCatalog,
    pub(crate) reasons: Vec<String>,
}
pub(crate) async fn prepare_and_connect<Launch, BeforeOAuth, BeforeConnect>(
    request: PluginToolRequest<'_>, policy: ToolFailurePolicy,
    launch: Launch, before_oauth: BeforeOAuth, before_connect: BeforeConnect,
) -> Result<PreparedPluginTools, PluginToolError>
where
    Launch: FnMut(&PreparedPlugins) -> Result<PluginLaunchDecision, PluginToolError>,
    BeforeOAuth: AsyncFnMut(&PreparedPlugins) -> Result<(), PluginToolError>,
    BeforeConnect: FnMut(&PreparedPlugins) -> Result<(), PluginToolError>;
```

`AsyncFnMut` 只表达候选接口的异步约束；最终可用已有具名方法或局部闭包减少参数，三个回调及新命名类型均不要求保留。先保留下面列出的事实写入、OAuth 与 MCP 启动顺序，再选择最少代码的共用方式；不新增依赖注入容器。PluginLaunchDecision 只表达现有的“可在 sandbox 启动 / 无需进程 / sandbox 原因已记录需跳过 stdio”三种分支，携带原来的 sandbox、workspace、readonly_inputs。它不是新的持久事实。

管线固定步骤：

1. 按 policy 把 bindings 分为 Pilot 的有序单项组或 Node 的一整个组；每组只调用一份准备代码。准备 P 覆盖 definition→checked authorization→view/mount/disclosure→owner configuration/MCP 展开；返回前 drop P。
2. 将本组披露加入目录。Pilot 原行为是在 OAuth/连接失败前已经把定义加入目录，必须保留“可发现但无工具”的状态。
3. launch 回调建立本次沙箱/挂载。Node 使用现有 prepare_tool_launch；Pilot 仅首次 stdio 初始化一次共享 sandbox，HTTP-only/fixture 不强求它。
4. before_oauth：Node 在启动外部服务前 retain_plugin_configuration；Pilot 空实现。配置身份不因抽管线改写。
5. 同一份 authorize_oauth，仍使用准备时捕获的 owner/network/cancellation。Pilot cancellation 的 select 与 break/cleanup 保留；Node 保留当前 authorize_oauth 的可取消机制，不顺便重写错误优先序。
6. before_connect：Node 在 OAuth 成功后、MCP assembly 前 remember_plugin_observation；Pilot 空实现。这里列出的扩展点只为展示已写事实和外部效果的先后关系；最终接口可以精简，但不能把观察移到连接成功之后。
7. 调原 assemble/assemble_with_sandbox_for_execution；成功后 attach_tools 同一披露目录；Node 一组故只返回一个 port，Pilot 按原顺序返回多个 ports。
8. 返回原调用方，由其加 NodeTools/channel/conversation/disclosure 或 Pilot 的固定管理工具。共用服务不调用这些适配模块。

### 失败策略矩阵（保持现状）

| 失败点 | Pilot PerPluginSoft | Node BatchHard |
| --- | --- | --- |
| 冻结 Session、selection P、工具环境不可用 | 整体失败 | 整体失败 |
| 单 Plugin 当前定义/view/配置准备失败 | 追加原 reason，继续下一项 | 立即返回原类型错误，不启动本组 MCP |
| Pilot stdio sandbox 创建失败 | 只记录一次原因；后续 stdio 跳过；HTTP-only 和 fixture 继续 | Node prepare_tool_launch 失败立即返回 |
| input_mount 的 sandbox grant 非法 | 现有 `?` 是整体失败，保持 | 整体失败 |
| OAuth 失败 | 该 Plugin reason，继续；已插入披露仍可查 | 硬失败，保留原错误类别 |
| MCP assembly 失败 | 原 `Plugin <id> MCP tools are unavailable` 原因，继续 | 硬失败，保留 PreparationError 的 known/unavailable 区别 |
| 取消 | 整体失败；关闭之前成功 ports，保留 cleanup 失败/活动引用文字 | 使用现有连接绑定的取消/cleanup 收束，不新增重试 |

不能把 Node 改成逐 Plugin 启动再聚合：其批量配置具有跨 Plugin server_key 冲突检查、全量授权检查和观察时点。不能把 Pilot 改成全量 prepare：一个无效 Plugin 会阻止其它可用 Plugin，与当前软失败冲突。policy 必须同时决定分组粒度和错误传播，不能只决定是否吞错。

## 8. 实施切片与验收边界

设计确认后依次：①搬 catalog/Library factories 与原 P 重试/R 生命周期，不改调用时点；②收口 checked_definition 与三处声明归一化，删除副本，直接复用原 plugin_mounts::resolve；③搬 prepare/connect 共用管线，删掉 Pilot/Node 旧重复步骤；④更新 application 列表用例依赖并检查围栏下降。

新机制的替代关系：PluginLibrary 替代 63 个分散构造；ReadCatalog/Publication 替代现有各处自行选择的原 guard/lease 构造；checked_definition 替代各处 read+compare；mount_declarations/merge_declarations 替代 3 处声明归一化，原 plugin_mounts::resolve 保持唯一实现并直接复用；tools 管线替代两条 prepare→OAuth→assemble→attach 的实现。没有新存储或权威来源。

本次执行搜索和读取，并用 `rustc --edition=2024 /tmp/anchor-host-h4-count.rs -o /tmp/anchor-host-h4-count` 编译一个调用 H0 共用扫描器的临时盘点程序，运行 `/tmp/anchor-host-h4-count` 得到 174 文件、46/17 构造、26/11 含构造文件、29 个去重文件。源码与可执行产物均在 /tmp；没有运行 cargo 测试，也没有声称 provider 端到端通过。H4 实施后需按任务书运行 Plugin/Pilot/Node 既有断言、完整 Host tests、clippy/fmt，并检验构造生产引用为模块外 0、layer fence 下降、Kernel diff 空；O→R/P 的同步策略变化不在这个设计中隐含授权。
