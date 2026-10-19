# Codex / OpenAI：大量 Skills 和 MCP 工具的渐进式披露

研究日期：2026-10-13。只读研究，没有修改 Anchor 的应用、配置或部署，也没有进行模型或业务调用。

## 证据范围

- 按 OpenAI Docs 技能先请求官方文档。`https://developers.openai.com/codex/skills/`、`https://developers.openai.com/api/docs/guides/tools-tool-search`、`https://platform.openai.com/docs/guides/tools-tool-search` 实际请求都返回 **403**。未将这些网页或搜索摘要当作已核实依据。
- 依照研究任务授权，转而读取 **OpenAI 第一方公开 Codex 源码**。最新正式 release API 实际返回 **`rust-v0.162.1`，2026-10-09T19:44:51Z，prerelease=false**。[Release](https://github.com/openai/codex/releases/tag/rust-v0.162.1)。该 release 的关键源码已下载，文件名前缀 `codex_release_`。
- main 分支另固定为 **`12cca49c81c8aa6a37a5cc4e39964ee3b70f9458`**，文件名前缀 `codex_`。这是 main SHA，**不是 release SHA**。release tag 的 peeled commit 尝试 `git ls-remote` 后 20 秒超时，未取得；因此 release 证据以已实际读取的 tag 内容为准，不混写 commit。
- “release 源码具备该机制”不等于远端 Codex 产品的所有账户、模型和界面都已开放。官方文档被阻断，没有进行安装运行或真实 Provider 验收。不能凭源码给出模型支持矩阵、API 可用性或无条件规模保证。

## 主要判断

Codex 的答案也不是把所有 Plugin 摘要持续塞入一个固定文本块。它把 **Skill 指令披露**和 **工具 schema 披露**分开：Skill 目录受预算约束，选中后才读正文及关联文件；支持原生搜索协议的模型则只先获得 `tool_search`，由客户端检索候选并在工具结果里返回完整 schema，下一次模型请求才获得相关调用能力。

但两个层面的规模策略并不等价。工具搜索可以在完整的宿主目录上检索而不把它全给模型；Skill 目录超额省略后，host 文件 Skills 的自动可发现性不能被无条件保证。检索本身也没有 100% 召回保证。

## 1. Skill 目录、正文及极端目录大小

正式 release 的 [render.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/render.rs#L19) 定义：

```rust
const DEFAULT_SKILL_METADATA_CHAR_BUDGET: usize = 8_000;
const MAX_CONFIGURED_SKILL_METADATA_TOKEN_BUDGET: usize = 10_000;
const SKILL_METADATA_CONTEXT_WINDOW_PERCENT: usize = 2;
```

- 未显式配置时，预算是已知上下文窗口的 **2%**；窗口未知则回退 **8,000 字符**。
- 显式配置 `max_context_tokens` 时，上限被压到 **10,000 token**。注意这个上限是显式 override 的上限，不能误说默认 2% 路径也总被 10,000 截断。
- 每条目录先限制描述长度到 **1,024 字符**，目录包含名称、描述、来源定位符；路径根别名减少重复长路径。
- 当全部完整条目能放下时全部显示；放不下但所有名称+定位符能放下时，对描述分配剩余预算；若连最简条目都放不下，就按既定顺序保留部分最简条目，**省略其他条目**。[分配逻辑](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/render.rs#L331)。
- 所以即使“只是 metadata”，成本仍随名称、定位符、描述和数量增长；预算最终通过省略来有界化，不能把全量名字表说成常数成本。
- CoreCompatible 和 ExtensionCompatible 对省略提示不同，前者不在模型目录中加 omission notice，后者会添加；另外有用户 warning。不能声称所有来源都一定在模型文本里明确显示了省略数量。

正式 release 的 [catalog_prompt.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/catalog_prompt.rs#L9) 指令明确写道：

> “How to use a skill (progressive disclosure)”

> “After deciding to use a skill, the main agent must read its `SKILL.md` completely before taking task actions.”

> “Progressive disclosure applies to selecting relevant files, not partially reading a selected instruction file. Do not load unrelated references, scripts, or assets.”

这里的渐进披露是先选 Skill，再按路由选关联文件；选中的规范文件要完整阅读。不能以减少 token 为由只读一段正文然后猜测其余约束。

### 被省略的 Skill 是否仍可达

- **Executor / Cloud 来源**：正式 release 提供 `skills.list` 与 `skills.read`，list 每页最多 **20 条**，使用 `next_cursor`，响应还受字节预算限制。list 按 authority 和 `is_model_visible()` 过滤；正文 read 从完整 catalogue 查找 enabled package，**不依赖目录渲染是否把它包含进去**。[list](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/tools/list.rs#L32)、[read](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/tools/read.rs#L83)。因此被预算省略的 enabled package，在这两种来源下仍可经分页发现、按标识读取。
- 这些工具的 authority 是 Cloud / Executor，且 [skill_tools](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/tools/mod.rs#L62) 仅当 cloud provider 可用或存在 executor query 时暴露。**不能说所有 host 文件 Skill 都自动有同一搜索 API**。host 文件正文走已列出的本地路径；代码没有证明被省略且用户也不知道路径的 host Skill 仍能被 Agent 稳定发现。
- list 是分页枚举，**不是语义搜索**。没有证据表明稳定版对任意数量 Skill 都自动做相关性检索。
- 正式 release 的 [dynamic_skill_selector.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/dynamic_skill_selector.rs#L46) 已有 BM25、字符 ngram、LRU、RRF 等 cheap selectors，但注释明确：

> “Selects likely-relevant skills without changing the model-visible catalog.”

> “cheap enough to run in shadow mode on every turn.”

这只能证明正在评估候选筛选，**不能当作稳定产品已用相关性动态筛 Skill**。

## 2. MCP / Apps 的工具披露与真实调用协议

正式 release 的 [mcp_tool_exposure.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/mcp_tool_exposure.rs#L86) 将正常 MCP 工具在支持搜索时注册为 `ToolExposure::Deferred`，否则注册为 `Direct`。

[spec_plan.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/spec_plan.rs#L392) 依据 **`model_info.supports_search_tool`** 决定是否加搜索工具。后面的 [build_model_visible_specs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/spec_plan.rs#L614) 只把 `exposure.is_direct()` 的工具放入初始模型工具集合，deferred schema 不被全部提前发送给模型。

`tool_search` 由客户端执行，而不是本段实现里由 OpenAI 服务端扫描 MCP。正式 release 的 [tool_search_spec.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/handlers/tool_search_spec.rs#L92) 明确：

> “Searches over deferred tool metadata with BM25 and exposes matching tools for the next model call.”

```rust
ToolSpec::ToolSearch {
    execution: "client".to_string(),
    // ...
}
```

[搜索实现](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/handlers/tool_search.rs#L164) 用本地 `bm25`、`Language::English` 建词法索引，query 不能为空、limit 需大于 0；匹配候选后 `take(limit)`，返回 `Vec<LoadableToolSpec>`。默认 8 的常量已在固定 main [tool_discovery.rs](https://github.com/openai/codex/blob/12cca49c81c8aa6a37a5cc4e39964ee3b70f9458/codex-rs/tools/src/tool_discovery.rs#L7) 实际读取；release 的 spec 测试也用默认 8 描述。该 handle 路径没有看到将 Agent 自选 limit 硬夹到统一小上限的逻辑，不能当作严格常数输出预算的完整范例。

实际原生链路是：

```text
初始 tools：核心直接工具 + tool_search
    → 模型发 tool_search_call
    → 客户端 BM25 检索当前 deferred 注册项
    → Responses tool_search_output：完整 namespace/function spec、description、parameters
    → 同一 Turn 的下一次模型请求
    → 模型发正常函数调用，Host MCP runtime 执行
```

正式 release 的 [ToolSearchOutput](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/context.rs#L218) 直接序列化到 `ResponseInputItem::ToolSearchOutput { status: "completed", execution: "client", tools: ... }`。**不是只返回名字的普通工具结果，也不是 Anthropic 的 `tool_reference`**。稳定的名称/namespace 标识和完整 input schema 随搜索结果返回，runtime 则早已存在于宿主 registry。

源码不是往初始全部 tools 参数里持续追加另一份工具清单；搜索结果作为原生 Responses item 参与下一次请求，由支持该协议的模型/API 理解。普通 OpenAI-compatible Provider、DeepSeek 或 Anchor 固定 Goose 的调用接口不能默认支持这个原生 item。Anchor 可借鉴“检索→完整参数→执行”的语义，不能不经适配直接发送 Codex wire format。

## 3. 权限、索引成本及更新

- [MCP exposure](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/mcp_tool_exposure.rs#L80) 在注册前区分正常 MCP 和 Apps。普通 MCP 有 `tool_is_model_visible`；Apps 还有 connector ID 与 `AppToolPolicyEvaluator.enabled` 过滤。
- 搜索 [handler cache](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/handlers/tool_search.rs#L75) 只收 registry 内 exposure 为 deferred 的项，并以 immutable runtime identity / dynamic metadata 判定是否复用索引；目录变化才重建，不每轮无差别重复构建。
- 代码模式嵌套搜索还会对本次 step 的 router policy 过滤，然后再限数量。[search_code_mode](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/handlers/tool_search.rs#L291)。这是重要顺序：先筛当前可用集合，再选 Top K。
- **延迟模型 schema 不等于延迟 MCP 连接或省去 Host 清单成本**。已经存在于 `McpBinding.tools()` 的工具 metadata 才能注册和索引；本次未建立“Codex 所有 MCP 只在调用时连接”的证据。
- agent-plugin 来源还有单 tool spec **8,000 bytes**、合计 **64,000 bytes** 的单独限制；超额 exposure=Hidden。[limits](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/mcp_tool_exposure.rs#L17)。不能把这样的隐藏描述为无限工具仍全部可发现。
- 常规搜索工具描述仍可包含所有 source 的短信息，但 aggregate cap 是 **512 KiB**。[source listing](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/tools/handlers/tool_search_spec.rs#L8)。它有上界，但很大，不是无论挂多少 Plugin 都仅几十 token 的目录。

## 4. 压缩、接续和状态

- 正式 release 的 [compaction](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/core/src/compact.rs#L79) 会通过 `build_initial_context_with_world_state` 重建 initial context 与比较 baseline，再组装压缩后的历史。
- 固定 main 的 [compact_remote_history.rs](https://github.com/openai/codex/blob/12cca49c81c8aa6a37a5cc4e39964ee3b70f9458/codex-rs/core/src/compact_remote_history.rs#L169) 截断某个 ToolSearchOutput 时会保留协议外壳、清空 `tools`。因此不应假设历史发现的完整 schema 永久保留；需要能够再次搜索。
- 正式 release feature 定义里，`tool_search` 和 `tool_search_always_defer_mcp_tools` 是兼容用的 Removed/no-op flags；注释说支持搜索时 MCP 默认 deferred。另一方面 **`DeferredToolWorldState` 仍为 UnderDevelopment，default_enabled=false**。[features](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/features/src/lib.rs#L1510)。不能把工具 namespace 的增量 world-state 目录当作 release 默认全部开启。
- 固定 main 有 tools world-state **4 KiB** 输出预算、每 namespace 描述 **250 字符**，优先保留名字，再分配描述，连名字也放不下会省略。[tools.rs](https://github.com/openai/codex/blob/12cca49c81c8aa6a37a5cc4e39964ee3b70f9458/codex-rs/core/src/context/world_state/tools.rs#L26)。它只省略模型投影，不修改持久的 namespace snapshot。
- 固定 main 的集成测试代码覆盖 unchanged/removed namespace、resume 不重复以及同 Turn MCP 恢复后更新目录；**本次只读测试，未运行**。[mcp_tool_exposure tests](https://github.com/openai/codex/blob/12cca49c81c8aa6a37a5cc4e39964ee3b70f9458/codex-rs/core/tests/suite/mcp_tool_exposure.rs#L1272)。不能因此声称 Anchor 或我们的模型端已通过恢复验收。
- Skills read 是可重复 API，omit cursor 可重新读取；分页 continuation 可以使用缓存快照。不存在证据表明“这个 Skill 曾读过就永远禁止重新读”。[read spec](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/ext/skills/src/tools/read.rs#L67)。

## 5. 对 Anchor 有直接价值的结论

1. **ToM 适合短入口与当前授权摘要，不能承担无限 Plugin/Skill 全量表。** 即使 source names 也需要预算；超限应给发现入口和遗漏说明，不能仅静默截尾。
2. **指令发现和工具发现分别处理。** 对 Skill 先查名称/用途/定位符，再完整读取选中的指令；对 MCP 工具必须得到完整参数 schema，才让 Agent 正确调用。
3. **查询完整授权集合、返回小批次。** 预算省略的 metadata 仍须在授权目录里可检索；不依赖模型已经知道那个被省略的名字。支持精确 Plugin/Skill ID 定位和分页可枚举兜底，避免只依赖 Top K 词法召回。
4. **索引是目录投影，不是新的权限事实。** 复用稳定定义和本轮工具准备结果；权限/可用性先过滤，实际执行时仍 Host 校验。目录变化失效索引；不能因“已发现”跳过授权变化。
5. **跨 Provider 用小而通用的契约表达搜索、读取、参数、调用。** Codex native `tool_search_output` 值得借鉴，但需要明确 Provider 支持；Anchor 可以沿现有 authorized MCP/Goose 工具执行通道实现同等语义，不另起 Agent loop。
6. **压缩后仍能重复发现、读取及获取 schema。** 原生框架负责历史和 compaction；Anchor 应确保发现入口始终可用、标识稳定、读取权限和准备结果准确，不维护永久“已加载”阻断标记。
7. **给规模和召回分别验收。** 目录字节/token、工具 schema、Host 索引/MCP 启动成本，以及多语言检索、同名歧义、被省略能力发现、权限撤销和压缩后重读，都要单独衡量。Codex 目前的 English BM25、shadow Skill selectors 和硬隐藏上限都不能证明任意数量 Plugin 的必然成功。

## 未验证

官方 Skills 与 Tool Search 最新文档正文、远端产品发布范围、具体模型/API支持矩阵、release peeled commit、完整权限运行实测、极端规模检索召回以及真实模型压缩后恢复均未验证。主线的设计建议是对已读取机制的借鉴，不是 Anchor 当前实现或验收状态声明。
