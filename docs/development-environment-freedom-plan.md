# 开发环境自由度：专项计划

编制日期：2026-10-13。状态：**待 review，未实现，未授权实施**。

本文是讨论产出的实施方向与验收要求。它**不是**进度台账；实际交付、失败和证据只记 [开发台账](pilot-development-plan.md)，当前实现以 [architecture.md](architecture.md) 为准，目标与不变量以 [product-architecture.md](product-architecture.md) 为准。现行依赖标准见 [plugin-dependencies.md](plugin-dependencies.md)，本计划**不修改**该标准的不变量，只在其第 3 条（每插件版本一份视图）的**边界之外**新增一层。

## 1. 问题陈述

Pilot 现在能在 Session 工作区写代码、跑命令、改代码立即生效。但它在"写 Python"这件事本身最常用的动作上受阻：

| Pilot 实际需要 | 现在能否做到 | 依据 |
| --- | --- | --- |
| 按标准 Python 项目结构组织代码（`src/` 布局） | **否** | 全仓库无 `editable` 实现；`pyproject.toml` 只出现在候选文件清单与 RSI 证据清单里，无安装处理 |
| `pip install -e .` | **否** | Pilot 命令别名只有 `python` / `python3` / `python3.<minor>`，无 `pip` |
| 快速试一个包 | **否**，须走完整 prepare 流程 | `development_prepare` 是全有或全无的用例 |
| 用 `requirements.txt` 当唯一声明 | **否**，声明是工具内联数组 | `development_prepare` 的 `requirements` 是 JSON 数组参数 |
| 不同项目用不同包版本 | **能** | 每项目独立 scope 目录 |
| 改源码后不必重新取包 | **能** | 准备事实绑定求解结果，不绑定源码摘要 |

**根因不是实现缺陷，是规则的适用范围被放大了。**

宿主有一组正确且必要的规则：**每个插件版本一份视图，视图只读、有身份、被引用保护**。它的理由是硬的——一个进程只能看到一个版本的同一个包（标准「边界与硬约束」有实测支撑），所以多版本共存只能靠"一进程一视图"。这条推理没有问题。

问题在于：**这套规则被同样应用到了开发路径上，而开发路径从不需要"两个版本同时存在"。**

开发的性质是**反复改主意**：换一个 pytest 版本、加一个 requests、试一试不行删掉。任意时刻只需要一个版本。开发路径付了"多版本共存"的全部成本，却一次都没用到这个功能。

具体代价：

1. **每次依赖变化生成一个有身份的新视图目录。** 开发视图的身份绑定求解结果（`<scope>/<project>/<resolved_view_key>/view`），依赖集合每变一次就是一个新目录，旧目录进入被引用保护状态。
2. **不可变带来的是"被保护的垃圾"，不是"干净"。** 一个不被任何东西引用的开发视图，既不能被删除（引用保护），也不是事实（没有用户）。
3. **为了维持视图的不可变性，放弃了 pip 的全部能力**，而不可变性在开发阶段不产生任何保护——见第 3 节。

## 2. 目标与非目标

### 目标

- Pilot 能用 `pip` 在**项目自己的环境**中自由安装、升级、删除包
- 支持 `src/` 布局、`pyproject.toml`、`pip install -e .`
- 项目目录中的 `requirements.txt` / `requirements-dev.txt` 成为唯一声明来源
- 开发环境可丢弃、可重建，不产生需要人工回收的资产
- **下面四条底线一条不减**

### 底线（本计划的验收前提，任何改动不得违反）

1. **插件依赖永不受开发活动影响。** 开发路径装、删、升级任何包，已安装插件的视图与其运行结果不变。
2. **正式插件 == 测试过的源码。** 由交付时的源码摘要核对保证，不由环境不可变保证。
3. **同一 Python 版本只有一份解释器**，且开发路径不可写它。
4. **共享存储的字节，开发路径不可达。** 不是"不可写"，是**不可寻址**。

### 非目标

- 不改共享存储的（包、版本）组织与内容寻址
- 不改插件视图的形态、身份与只读语义
- 不改回收判据（标准不变量 8）
- 不改交付门禁（`plugin_prepare` / `plugin_test` / `plugin_install` 的摘要核对）
- 不改权限边界、沙箱策略、网络授权模型
- 不改 Run 记录格式
- 不引入命名环境、第二套依赖资产或通用开发平台

## 3. 关键论证：不可变性在开发路径上不产生保护

这是本计划的立论基础，请 review 时重点检查。

"测过的 == 发布的"这件事现在由**两道**机制共同保证：

| 机制 | 保护的命题 | 是否必要 |
| --- | --- | --- |
| 开发/候选视图不可变 | "环境没被改造过" | **否** |
| 交付时源码摘要核对（`plugin_prepare` → `plugin_test` → `plugin_install`） | "装的是测过的那份**源码**" | **是** |

第二行才是真正的保证。理由：候选准备、测试、安装共用摘要，`plugin_test` 之后源码变化必须按新摘要重测，`plugin_install` 在原生确认前后两次核对摘要。**这套机制证明的是源码身份，与环境目录的身份无关。**

一个环境再怎么不可变，也证明不了源码没被改过；反之，环境再怎么可变，只要交付时核对源码摘要，一致性就成立。

结论：**不可变性在开发路径上承担了一个它本来就不该承担的角色。** 本计划收回这个开销，同时保留第二行的全部效力。

## 4. 三层模型与核心不变量

```
第 1 层  共享存储    内容寻址 · 不可变 · 一份字节        ← 真相
第 2 层  插件视图    symlink 进存储 · 只读 · 有身份      ← 承诺
第 3 层  开发环境    私有拷贝 · 可变 · 无身份            ← 草稿
```

**核心不变量：**

> **可变性止于第 3 层；且第 3 层与第 1 层之间是拷贝，不是链接。**

拷贝（reflink / copy-on-write）这一条是物理隔离的来源。有它，第 3 层的 `pip uninstall`、升级、`--target` 写入**都不可能触碰第 1 层字节**——不需要任何检查代码，因为违规不可表达。

**"干净"的重新定义：** 干净 ≠ 不可变。干净 = **环境里不存任何无法从"声明 + 存储"重新推导出来的事实**。第 3 层是纯投影，所以可随时删除、随时重建，永远不是垃圾，因为它从来不是真相。

### 现状与目标的对照

| 层 | 现在 | 目标 |
| --- | --- | --- |
| 共享存储 | 内容寻址、不可变 | **不变** |
| 插件视图 | symlink 进存储、只读、身份 = 声明摘要 | **不变** |
| 开发环境 | 复用 `PreparedView`：symlink 进存储、只读、身份 = 求解结果、版本化 | 私有拷贝、可变、无身份、每项目一份 |

## 5. 现状事实（供 review 核对）

以下是编制时通过读代码确认的事实，实施前须重新核对（符号可能已变）。

| 事实 | 位置 |
| --- | --- |
| 视图 = `pyvenv.cfg` + `bin/python`（symlink 到部署解释器）+ 每个已安装 distribution 一个指向共享存储的 symlink；**read-only content，nothing is copied and nothing is writable** | `plugin_view.rs` 模块文档 |
| 开发视图与候选视图**复用** `PreparedView::for_candidate(view, store, declaration_digest)` | `plugin_view.rs`、`application/development_dependencies.rs` |
| `for_candidate` 把 `view` 本身与 `referenced_store_directories` 全部作为 `ReadOnlyInput` 授权 | `plugin_view.rs` |
| `anchor_run(project=...)` 已经：把 `view/bin` 前置进 PATH、`python` 别名重写到 `bin/python`、目录内可执行文件解析到 `bin/<command>`、设 `PYTHONNOUSERSITE=1` | `node_tools.rs` |
| Pilot 命令别名只有 `python` / `python3` / `python3.<minor>` | `tool_environment.rs` `pilot_command_aliases` |
| 开发视图路径：`<scope>/<project>/<resolved_view_key(fact.resolved)>/view`，身份绑定**求解结果** | `application/development_dependencies.rs` |
| 开发视图有持久归属与引用，回收按标准不变量 8 保护 | `anchor-library/src/development.rs`、`reclaim.rs` |
| 回收的触发点只有插件安装/卸载与显式 API 调用，**不含 Session 删除** | `anchor-library/src/lib.rs`、`api.rs`、`application/plugins.rs` |
| 共享存储在 Pilot 命令沙箱的挂载列表中**无任何引用** | `tool_environment.rs`（搜 `store` 零命中） |
| `build.rs` 的沙箱**结构性禁用网络**（适配器未装 `allow_network`），安装用 `--no-index --find-links --require-hashes` | `dependency_provision/build.rs` |
| 依赖声明是工具内联数组，非 `requirements.txt` | `pilot_tools.rs` `development_prepare` schema |
| **无 editable / `pyproject.toml` 安装支持** | 全仓库搜 `editable` 无实现命中 |

**注意最后一条的措辞**：这是"没有找到支持"，不是"证明不支持"。实施前必须自行确认。

## 6. 实施切片

顺序为 D1 → D2 → D3 → D4。**D1 单独就有价值**（它消除第 8 节的风险），D2 是核心收益，D3/D4 是可用性收口。

### D1：开发环境的字节私有化（拷贝，不链接）

**交付行为：** 开发环境中的包与共享存储**不共享 inode**。开发路径的任何写入都不可能到达共享存储。

**实现要点：**

1. 为开发视图引入独立于 `PreparedView` 的装配路径，或在 `for_candidate` 增加显式模式参数。**不要**让插件视图也走拷贝——那会摧毁共享存储的全部收益。
2. 拷贝使用 reflink（XFS 上 `cp --reflink=auto` 或等价方案），退回普通 copy。`UV_LINK_MODE=copy` 已在 `build.rs` 中使用，可复用同一策略来源。
3. 共享存储仍需**只读授权**给开发沙箱——因为它是拷贝的来源；但沙箱内的**写入目标**是开发环境目录，不是存储。
4. 拷贝完成后校验：开发环境中不存在指向共享存储的符号链接。

**验收条件：**

- 开发环境内 `find -L` 走到共享存储的路径为零
- 在开发环境中对某个包执行删除/改写后，共享存储对应目录的 mtime、内容与 inode 全部不变
- 插件视图仍为 symlink 形态，插件运行结果与改动前逐字节一致
- 拷贝的空间占用在 reflink 生效时接近零（用 `df` / `fiemap` 观察，不用 `du`）

### D2：开发环境可变、无身份、每项目一份

**交付行为：** 依赖变化不再产生新目录；每个项目一个可变的开发环境。

**实现要点：**

1. 路径从 `<scope>/<project>/<resolved_view_key>/view` 改为稳定的每项目路径（如 `<scope>/<project>/env`）。
2. **移除开发视图的"当前准备事实"版本化语义。** 正式视图与候选视图保持原样。
3. 开发视图**不再进入回收的引用保护集合**。它是可丢弃的草稿，不是需要保护的资产。**注意**：这是一处语义变更，须确认不会让共享存储里只有开发引用的包版本被误删——见第 7 节。
4. `development_prepare` 的语义从"生成一份有身份的准备结果"变为"确保该项目的环境存在且与声明一致"；语义变化必须在工具描述中如实反映。
5. 源码编辑仍不触发重新准备（保持现有正确行为）。

**验收条件：**

- 同一项目连续多次 `development_prepare`，环境路径不变，不产生新目录
- 依赖增删后环境原地更新，旧内容不残留为独立目录
- 两个项目各自的包版本互不干扰
- 删除项目目录后不留下需要人工回收的资产

### D3：开发环境的 pip 与 PATH

**交付行为：** `anchor_run(project=...)` 中 `pip install` / `pip uninstall` / `pip install -e .` 可用，且只影响该项目环境。

**实现要点：**

1. D1 之后开发环境已有 `bin/`。需要确保它对开发环境是**可写**的（插件视图仍只读）。
2. **不要把共享解释器的 pip 变可达。** 那是底线 3。
3. 现有的 `python` 别名重写与 PATH 前置逻辑（`node_tools.rs`）可复用；`pip` 与 console script 需要走同一解析路径。
4. `PYTHONNOUSERSITE=1` 保留，防止用户级 site-packages 泄漏。
5. **网络**：pip 需要网络，必须沿用现有 Turn 冻结的任务联网授权。禁止联网时 pip 失败须如实返回，不得绕过。
6. `pip install -e .` 的 editable 机制需要 index 或本地路径解析；确认沙箱内路径与 `sys.path` 行为正确。

**验收条件：**

- `pip install` 后包可 import，且共享存储字节不变
- `pip uninstall` 后**已安装插件仍能正常运行**（底线 1 的直接验证）
- `pip install -e .` 在 `src/` 布局项目中生效，源码编辑立即被 import 到
- 未授权联网时 pip 明确失败，不静默降级、不回退宿主网络
- 共享解释器的 site-packages 仍不可写（用直接写入尝试验证被拒绝）
- 两个项目装同一包的不同版本，各自的 `python -c "import x; print(x.__version__)"` 正确

### D4：`requirements.txt` 作为声明来源

**交付行为：** 项目目录中的 `requirements.txt` / `requirements-dev.txt` 是唯一声明来源，替代内联数组。

**实现要点：**

1. `development_prepare` 从项目目录读取声明文件；内联参数保留为兼容路径，但须明确二者不一致时的行为（建议：显式报错，不静默取其一）。
2. 路径解析复用现有 workspace 目录约束（`workspace_directory`，拒绝绝对路径与 `..`）。
3. `requirements.txt` 的解析与现有 `ResolvedRequirements` 语义对齐；哈希校验的处理须明确——见第 8 节。
4. **不改变**正式 `plugin.json` 顶层 `dependencies.python` 的契约。开发声明不自动进入正式声明（现行标准不变量 1）。

**验收条件：**

- 修改 `requirements.txt` 后 `development_prepare` 反映变化
- 内联参数与文件并存时行为明确且可解释
- 开发声明不污染正式 `plugin.json`

## 7. 生命周期：需要一并处理的缺口

D2 会让开发视图失去回收保护，这会暴露一个现有缺口：

> **只有开发路径引用的包版本，其字节何时回收？**

现状：开发视图有引用保护，因此这些字节**永远不会**被回收。回收触发点只有插件安装/卸载与显式 API 调用，**不含 Session 删除**。

D2 之后，开发视图不再受保护，那么：

- **短期选择（推荐）**：接受。字节有界（下次装/卸插件时回收），且回收的"疑罪从有"原则保证不误删。**不要**为此新建机制。
- **可选增强**：`release_development_scope`（Session 删除路径，`pilot_host.rs` 已调用）末尾顺带跑一次回收。

**无论选哪个，必须回答这个问题并写入验收：会话删除后，共享存储中只有该会话引用的包版本会怎样？** 现在的答案是"保留"，D2 后的答案必须明确。

## 8. 风险与开放问题

| 风险 / 问题 | 说明 | 处理建议 |
| --- | --- | --- |
| **reflink 不可用的文件系统** | ext4 早期版本、非 XFS/Btrfs 无 reflink | 退回普通 copy；空间成本可接受，正确性不依赖 reflink |
| **符号链接残留** | `for_candidate` 现在会枚举 `referenced_store_directories`；拷贝路径若有任何残留 symlink，D1 的隔离即失效 | D1 验收必须包含"零可达 symlink"的显式检查 |
| **开发环境不受哈希校验** | 直接 pip 从索引取包 | **有意接受**。只影响开发；交付时按正式声明**重新求解并校验哈希**，开发环境的不受验证性被交付门禁隔离在外 |
| **hardlink 的误用** | 若用 hardlink 代替 reflink，`pip` 的原地改写会穿透到共享存储 | 明确禁止 hardlink；标准「边界与硬约束」已说明 hardlink 要求同设备且会共享 inode |
| **`requirements.txt` 与哈希** | 直接 pip 无哈希；正式路径有 `--require-hashes` | 开发路径不承诺哈希校验；交付路径不变。须在文档中如实区分 |
| **可复现性** | 开发环境不再可复现 | **这是目标，不是代价**——开发环境应当可丢弃，不应可复现 |
| **`pip` 作为新授权命令** | 新增可执行入口意味着新的权限面 | 必须确认它只能写开发环境目录，不能借 pip 触及宿主任意路径 |
| **并发** | 两个 Turn 同时改同一项目环境 | 复用现有 scope lease；须验证并发与取消行为 |
| **旧开发视图的处置** | D2 后旧路径的视图成为遗留 | 不迁移、不主动清理（与现行旧环境目录处置约定一致），但须在文档说明 |

## 9. 底线对照检查表（review 必须逐条回答）

| 底线 | 本计划如何守住 | 需要验证的证据 |
| --- | --- | --- |
| 1. 插件依赖不受开发影响 | D1 的拷贝隔断 + 插件视图形态不变 | D1 验收的 inode/mtime 检查；D3 的 `pip uninstall` 后插件仍运行 |
| 2. 正式 == 测试过的源码 | **完全不依赖本计划**；交付门禁一行未改 | 现有 `plugin_prepare`/`plugin_test`/`plugin_install` 摘要核对测试保持通过 |
| 3. 一份解释器、不可写 | 共享解释器挂载方式未改 | D3 验收的直接写入被拒 |
| 4. 共享存储开发路径不可达 | D1 的拷贝 + 零 symlink 检查 | D1 验收的零可达 symlink |

## 10. 影响的文档

实施后须更新（**实施前不得预改**）：

- [architecture.md](architecture.md)：开发视图的装配、路径与可变性语义
- [plugin-dependencies.md](plugin-dependencies.md)：确认第 3 条不变量（每插件版本一份视图）**未变**，并说明开发环境不在其适用范围内
- [pilot-initialization-and-development-plan.md](pilot-initialization-and-development-plan.md)：I2 的契约新增 pip 与 `requirements.txt`
- [plugins.md](plugins.md)：`development_prepare` 的语义变化
- [pilot-development-plan.md](pilot-development-plan.md)：追加推进记录

## 11. 接续与工作区保护

编制时工作区存在多条未提交改动（`experiments/`、`research_plugin_disclosure_20261013/`、`docs/wecom-persistent-run-development-guide.tmp.md` 等）。

- 不 `git reset`、不 `git checkout --`、不 `git clean`
- 不把无关改动纳入提交
- 实施前逐路径核对 diff

### 给 review 的重点

请优先审查以下三条，它们决定本计划成不成立：

1. **第 3 节的论证**："交付时源码摘要核对才是真正的保证，环境不可变不是"——如果这条不成立，本计划的基础不成立。
2. **D2 的语义变更**：开发视图失去回收保护后，共享存储中只被开发引用的包版本的命运必须明确（第 7 节）。
3. **D1 的隔离完整性**：任何残留的符号链接都会让"违规不可表达"退化为"违规没人检查"（第 8 节）。

### 未知项（不得当作已知）

- 是否真的不支持 editable 与 `pyproject.toml`：本文依据是"搜索无实现命中"，**属于未证实**
- reflink 在目标部署文件系统上的实际可用性：**未实测**
- 开发环境拷贝对磁盘的实际占用：**未测量**
- `pip` 作为新授权命令的完整权限面：**未审计**
