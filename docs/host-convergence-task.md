# Host 收敛任务书

目标：`anchor-runner-host` 的依赖方向与文档一致，每条规则只剩一份实现。不改用户可见行为、HTTP JSON、磁盘路径与字节、权限边界；行数下降是结果，不是指标。

本页是交给开发 Agent 的任务书。每项完成后由架构负责人验收，验收结论写回本页「状态」列，过程留在提交说明。

## 现状（2026-10-25，master `e3e8384`）

- `runner-host` 生产代码约 58k 行，是单个二进制 crate，约 70 个模块平铺在 `main.rs` 下，`pub(crate)` 在全 crate 可见。唯一有测试固定的边界是 [Run 访问围栏](run-access-facade.md)。
- 下层反向依赖上层：非入口模块对 `crate::application` 有 73 处引用（20 个文件），对入口适配模块（`api`、`pilot_tools`、`node_tools`、`channel_tools`）有 34 处（14 个文件）。约 50 处是 `application::metadata` 的读写，它实际是 Run 元数据存储，只是放在了 `application` 下。
- 同一规则存在多份实现，部分已经分叉：
  - Plugin id 有两种规则。Library、graph-host、runtime authoring 和 `api.rs:660` 要求首字节为字母或数字；`pilot_tools::identifier` 不要求，却被凭据和插件配置拿去校验 Plugin id（例如 `credential_store.rs:353`、`plugin_configuration.rs:63`）。
  - 上下文窗口：设置层 `model_configuration.rs:232` 接受 0，运行时 `goose_acp/configuration.rs:390` 才拒绝。
  - 「`required` 只对 Pilot Graph 生效」及挂载默认值写了 3 份：`api.rs:641-652`、`node_host/mounts.rs:400-450`、`node_host/turn_plugins.rs:128-160`。
  - 生产代码中 `FilePluginCatalog::new` 有 46 处、`Library::new` 有 17 处，共分布在 29 个文件（H4 设计实测）。目录租约在每个调用点各自决定。
  - Pilot 与 AgentNode 各写了一条工具组装管线：`pilot_tools.rs:125-310` 与 `node_host/preparation.rs:41-170`，流程都是 prepare_plugins → authorize_oauth → assemble_with_sandbox → attach_tools，只有失败策略不同。
  - `sha256(durable_key)` 文件名主干有 7 份；`goose_acp::pilot::directory`（不跟随符号链接的持久建目录）被 14 个文件借用。
  - `ANCHOR_BWRAP`、`ANCHOR_RUNNER_LIBRARY_ROOT`、`ANCHOR_RUNNER_CATALOG_ROOT`、`ANCHOR_RUNNER_STATE_ROOT` 在 `main.rs` 之外仍被重复读取，与 `main.rs:248` 的注释相违。
- 兼容旧格式的代码约 1–1.5k 行，不是本任务的对象。

## 已定的架构决定

执行中不必再请示，照此执行；遇到与之冲突的事实，停下并在交回里说明。

1. **层次。** 组合根是 `main.rs`、`bin/` 与 `execution.rs`（后者是共享 Runner 的装配，与 Run 围栏的运行时豁免一致）。入口适配是 `api*`、`pilot_tools*`、`node_tools*`、`channel_tools.rs`。用例层是 `application*`。其余都是 Host 服务，不得引用用例层或入口适配；用例层不得引用入口适配。
2. **Run 元数据是存储，不是用例。** `application::metadata` 及其所需的存储助手下沉为顶层 `run_metadata` 模块，使用自己的错误类型；`RunApplication::graph_identity` 改为同模块的自由函数。
3. **Plugin id 的唯一规则**是「首字节为 ASCII 字母或数字，其余为 `[A-Za-z0-9_.-]`」，权威实现放在 `anchor-runtime`（Library、graph-host、Host 已经依赖它），其余副本改为调用它。名称规则（非空、非 `.`/`..`、`[A-Za-z0-9_.-]`）仍用于 Graph 名等，同样只留一份。Library 从不安装首字节不合规的 id，收紧不影响已安装的插件。
4. **配置只在组合根读取。** 上面四个变量只在 `main.rs` 读取，以参数或配置结构向下传递。模型字段只有一个校验函数，保存和运行都调用它，保存时就不接受 0。
5. **Plugin 领域的唯一主人**是一个新的 Host 服务模块（建议名 `plugin_library`）。它负责：Library/目录根、带正确租约的 catalog 与 Library 构造、读定义并重验摘要、按 Graph 声明与 Pilot Graph 求挂载声明。`application/plugins/catalog.rs` 的列表用例建在它之上。Pilot 与 Node 共用一条工具组装管线，失败策略作为参数传入。
6. **不拆 crate。** 本轮只理顺模块依赖；全部任务完成后再评估是否值得拆 crate。

## 任务

| 任务 | 规模 | 依赖 | 状态 |
| --- | --- | --- | --- |
| H0 依赖方向围栏 | S | — | 已验收（`a0de84b`）；实测基线 58 / 35 / 1，[证据](../experiments/host-convergence/h0/verification.md) |
| H1 共享原语与已分叉规则 | M | H0 | 已验收（7dd8204）；依赖计数 58 / 23 / 1；本机已安装 Plugin id 均合规，[证据](../experiments/host-convergence/h1/verification.md) |
| H2 Run 元数据下沉 | M | H1 | 已验收（ebc9919）；依赖计数 25 / 23 / 1，服务 → 用例减少 33（按下文确认口径），[证据](../experiments/host-convergence/h2/verification.md) |
| H3 配置只在组合根读取 | M | H2 | 未开始 |
| H4 Plugin 领域单一主人 | L | H3；调研可提前 | [设计](../experiments/host-convergence/h4/design.md)已确认，按下文三项修改实施；未开始 |
| H5 剩余反向依赖清零 | M | H4 | 未开始 |

规模：S 约半天，M 约 1–2 天，L 约 3–5 天（按 Agent 工作量估计）。

### H0 依赖方向围栏

- 新增 `tests/layer_fence.rs` 与 `layer-fence.txt` 逐文件上限，仿照 `run_access_fence`：沿用同样的扫描规则（排除测试、注释与字面量），可把词法器抽到 `tests/common/` 共用，`run_access_fence` 的行为不得改变。
- 计数三类引用：Host 服务 → `crate::application`；Host 服务 → 入口适配；用例层 → 入口适配。层的划分按决定 1 写死在测试里。
- 基线取当前实测值（上文的 73 / 34 / 1，以测试实际结果为准）。超过上限或清单外文件出现引用就失败；下降时提示同步降低清单，不自动改写。
- 验收：在临时副本里加一处违例，确认测试失败，再撤回。

### H1 共享原语与已分叉规则

- Plugin id 和名称规则按决定 3 各留一份。所有副本改为调用它，包括 `pilot_tools::identifier`、`api.rs:660`、`local_inputs.rs:241/250`、`application/graphs.rs:231`、graph-host `resources.rs:8`、runtime `authoring.rs:226`、Library `validate_id`。凡校验 Plugin id 的调用点改用 Plugin id 规则，其余仍用名称规则；分界逐个列在提交说明里。
- 新建中立模块（建议名 `host_fs`），放入不跟随符号链接的持久建目录（现为 `goose_acp::pilot::directory`）和 `sha256(durable_key)` 主干函数。7 处主干调用改用该函数，各自的前缀（`nf1-`、`fixture-` 等）保持原样，磁盘路径字节不变。`main.rs:70` 的 `create_durable_directory` 语义不同（跟随符号链接），保留并注明区别。
- 先写失败测试，再修：以 `-x`、`_x` 为 id 的 Plugin 在凭据与配置入口被拒绝。本机已安装插件的 id 是否都合规，由验收者核查；开发 Agent 不读取 `.local/`。

### H2 Run 元数据下沉

- 按决定 2 移动 `application::metadata`（339 行）。`execution.rs`、`assistant.rs`、`assistant_plugins.rs`、`node_host/*`、`conversation_resources.rs` 等处改为引用 `run_metadata`。`graph_identity` 的 8 处调用改为调用自由函数。
- 只移动，不改字段、不改文件格式、不改 save/load 语义。错误映射在 `application` 边界处做一次。
- 验收：`layer-fence.txt` 中 Host 服务 → 用例层的计数至少下降 40。实测纯元数据迁移降 33（58 → 25），经用户确认按此验收；剩余 25 处指向 Run 门面、RunApplication、用例错误和 Plugin 发布租约，留给 H3–H5。

2026-10-25 开发实测与范围确认：H1 基线为 58 / 23 / 1；纯元数据下沉后为 25 / 23 / 1，服务 → 用例实际减少 33，与原预估的 40 不一致。余下引用指向 Run 门面、Plugin 租约及其它用例接口。用户已确认保持 H2 范围、按减少 33 交回验收；剩余依赖按 H3–H5 处理，不为满足预估计数扩大本阶段。

### H3 配置只在组合根读取

- 按决定 4，四个变量在 `main.rs` 之外的读取清零（`tool_host.rs:307-308`、`tool_environment.rs:796`、`api.rs:166/195/259`、`pilot_tools.rs:91`、`goose_acp/pilot.rs:311`、`execution.rs:50/94`、`application.rs:341`）。测试里改环境变量的用例同步改为传参，不再依赖全局环境锁。
- 模型字段：抽出唯一的校验函数，设置保存和 Goose 配置都调用它；补一个「保存上下文窗口 0 被拒绝」的测试。开发 Agent 不读取 `.local/` 下的任何设置或凭据文件；本机已保存的设置能否被新校验接受，由验收者核查。
- 其余 `ANCHOR_*` 变量本轮不动，只在交回里列出 `main.rs` 之外仍有读取的清单。

### H4 Plugin 领域单一主人

- 第一步是设计说明，只读即可，可以和 H1–H3 并行：列出全部 catalog 与 Library 构造各自的用途和租约，对照决定 5 给出新模块的接口（函数签名级别），以及 Pilot 与 Node 共用管线的形状。设计说明交验收者确认后再动代码。
- 实现：
  - 挂载声明（含 `required && pilot` 与默认值）只在新模块求一次，`api.rs`、`node_host/mounts.rs`、`node_host/turn_plugins.rs` 改为调用它；
  - 读定义加摘要重验只留一份；
  - Pilot 与 Node 共用工具组装管线，Pilot 的「逐项软失败并记录原因」与 Node 的「硬失败」作为策略参数传入。
- 验收：
  - `FilePluginCatalog::new` 与 `Library::new` 在新模块之外的生产代码中为 0，或者每处保留的调用在设计说明里有理由；
  - 现有 Plugin、Pilot、Node 相关测试全部不改断言即通过。只能因为调用路径变化改测试的构造代码。

验收者对设计的确认（2026-10-25）：租约分类、构造清单、`checked_definition` 保留四种比较、挂载声明复用 `plugin_mounts::resolve`、失败策略矩阵均照设计执行。另有三项修改：

- **`Catalog` 只包装实际用到的方法**，不镜像 `FilePluginCatalog` 的全部 API。错误优先沿用现有类型（`GraphError`、`InstallError`）；只有调用方确实需要区分取消等情形时才加新枚举。目的是避免薄包装层本身变成新增行数。
- **工具管线不用回调，也不用策略枚举。** 共用的是步骤实现，不是调用顺序：拆成几个普通函数，例如「准备一组」「授权 OAuth」「连接并挂接披露」。Pilot 和 Node 各自按现有顺序调用，Node 自己的配置保留与观察记录写在调用之间。分组（逐项还是整批）和错误处理（记原因还是 `?`）留在各自调用方。设计里的时序约束照旧，接口形状以此为准。
- **验收基线以设计实测为准**：生产代码中 `FilePluginCatalog::new` 46 处、`Library::new` 17 处，在 `plugin_library` 之外降为 0。

### H5 剩余反向依赖清零

- 处理 H2–H4 之后 `layer-fence.txt` 剩下的计数。已知的大头：
  - `node_host` 自己构造 `RunApplication`（`node_host.rs:100,189`、`node_host/conversation.rs:228`），改为由组合根注入所需的小端口；
  - `durable_write.rs:238`；
  - `goose_acp/pilot_interaction*` 引用用例层的错误与前置条件类型，改为由入口适配转换；
  - `credential_store`、`tool_host`、`pilot_host` 对 `pilot_tools`、`api` 的引用。
- 目标是三类计数全部为 0。确有必要保留的，在本页逐项写出理由，并在测试里固定为命名例外。

## 共同要求

- 每项单独一个分支或工作树，从最新 master 开始，按依赖顺序合入。同一时间同一文件只有一个编辑者；H4 设计说明只读，不受此限。
- 不改：HTTP 路由与 JSON、磁盘路径与字节、Kernel（`git diff 9a5fa57 -- rust/anchor-kernel` 必须为空）、`run-access-fence.txt` 的上限（只许下降）。
- 测试只因模块路径或构造方式变化而修改，不改断言；如果确需改断言，说明这是行为变化，停下来交回。
- 每项交回时提供：
  - `scripts/test.sh -p anchor-runner-host`（H1 另加 `-p anchor-runtime -p anchor-graph-host -p anchor-library`）、clippy 与 fmt 的实际结果；
  - `layer-fence.txt` 前后的计数；
  - 生产代码行数前后的变化；
  - 偏离本页的地方及理由。
- 不读取 `.local/` 下的设置、凭据和运行状态；不部署、不推送。
- 遵守 [开发约定](../AGENTS.md)，包括措辞规则。

## 并行方式

H0 → H1 → H2 → H3 → H4 实现 → H5 依次进行：它们都要改 `api.rs`、`main.rs`、`node_host/`、`pilot_tools.rs` 等同一批文件，并行只会把时间花在合并上。

可以并行的只有两件：H4 的设计说明（只读），以及 H0 完成后由另一个 Agent 预先盘点 H5 的清单（只读）。总量约 8–12 个 Agent 工作日。每项交回后由验收者在当天核查。
