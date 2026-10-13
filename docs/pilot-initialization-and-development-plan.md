# Pilot 初始化与 Python 开发依赖：专项计划

编制日期：2026-10-12。用户已确认本文的两个需求；本文保存范围、实现契约和验收要求，实际进度、失败、证据与未验证边界只记开发台账。

[开发台账](pilot-development-plan.md) 是进度与证据的唯一来源；[当前架构](architecture.md) 描述已实现行为，[产品与系统架构](product-architecture.md) 描述目标。已有 Plugin authoring 的交付见 A207、A212，开发服务部署见 A213；这些旧证据不能关闭本专项，新包验证和实际部署必须分别记录。

## 1. 两个已确认需求

| 需求 | 用户结果 | 验收判据 |
| --- | --- | --- |
| I1 空机器上的 Pilot 初始化 | 安装 Anchor、配置模型后，用户仅通过 Pilot 完成基础环境、Plugin 和 Graph 的初始化；所需联网授权、凭据输入和启用操作均从对话发起 | 没有预装 Python/Rust 开发环境、没有预配置依赖来源、没有业务 Plugin/Graph 的隔离安装中，Pilot 能准备所需能力并完成可运行业务；无需编辑环境文件、手工登记路径或另开终端 |
| I2 Pilot 自由准备并使用 Python 开发依赖 | Pilot 通过 Anchor 准备自己选择的库和版本，在 Session 沙箱内用普通命令编写、运行、测试和修正代码，最终通过现有接口交付 Plugin | `anchor_run` 的脚本、测试命令实际使用准备好的依赖；安装后的正式 Plugin 在开发现场与 Session 删除后仍可运行 |

“空机器”指满足 Anchor 支持平台要求、已完成 Anchor 安装的机器。交付入口负责 Host、固定 Goose、网络中继、WebUI 和必要原生引导工具，以及沙箱所需的系统基础；Python、Rust 和第三方包按需准备。模型信息由用户提供，外部系统登录与业务凭据按任务需要通过 Pilot 引导提供。

I1 保留此前 Graph、Plugin 和环境全程由 Pilot 管理的目标：联网/挂载开关、外部服务配置、渠道绑定和需要的计划调度也应能在对话中完成。首个小型闭环不能冒充这些后续边界全部通过。

开发过程由 Pilot 自行决定实现、文件、测试和修正方式；Host 提供通用工具、共享基础能力、权限边界和交付接口。首期 Python/Rust 的既有范围保留。2026-10-13 用户认可的 [开发环境方案](development-environment-freedom-plan-revised.md) 将 I2 项目开发切换为私有可写 venv；候选、正式 Plugin 和旧项目兼容仍遵守共享只读视图规则，以下契约按此更新。

## 2. 实现接入与边界

| 领域 | 实现契约 | 不能据此推断的能力 |
| --- | --- | --- |
| Pilot 启动 | 完整平台使用内置管理指令，空 catalog 可启动；独立 Graph 与显式旧助手 Graph 保留原检查 | 平台 ready 不是业务 Graph 已运行或模型请求已通过 |
| Python 基础能力 | `RuntimeConfiguration` 统一解析；`runtime_prepare` 使用固定 uv，成功后发布 `state/managed/runtime-config.json`，后续执行使用同一解释器 | 显式部署字段不可覆盖；基础解释器不能被 pip 或候选代码修改 |
| Rust 基础能力 | 无预登记时使用固定 rustup-init 初始化；共享工具链通过官方入口管理并探测 | 不复制开发机工具链或 Cargo 凭据，不承诺所有 native crate 的系统依赖已齐全 |
| 获取与网络授权 | 发行包提供原生 `network`；`session_network` 和 `plugin_mount` 经原生确认，在当前 Turn 收束后接续 | Plugin 声明不能覆盖用户禁止；授权变化不扩充当前执行 |
| Python 包准备 | `development_prepare` 默认创建空私有 venv/pip、保留已有现场；显式 rebuild 只重建空环境，内联数组是 pip 安装请求 | 准备不证明依赖健康；开发测试通过不等于新改动候选已被测试或批准 |
| 普通命令 | `anchor_run(project=...)` 共用私有 Python/pip/PATH，支持标准项目文件、editable、脚本和 console script；旧项目默认保留只读视图 | 模型不能指定任意 Host 环境路径，不能写共享解释器、store 或正式视图 |
| 共享与回收 | 正式、候选和旧视图复用 Library 包字节；新项目仅共享解释器，scope lease 与项目锁保护私有现场，Session 删除统一释放 | 目录消失不等于引用释放；开发保护不冻结正式 Plugin 的历史版本 |
| 交付 | review/prepare/test/install 沿现有用例；对话内启用在下一 Turn 生效，正式文件与视图独立于开发根 | 空平台不虚构助手 Graph；显式旧助手的指令 Plugin 仍拒绝自修改 |
| 凭据与业务配置 | 专用输入保存 owner-bound 私有凭据；OAuth 复用 Library transaction，渠道与计划复用现有 Supervisor/ScheduleStore | 本地 fixture、配置或 token 流程通过不等于真实外部登录、企微消息和未来调度业务已通过 |

以上是接口与代码契约；真实 Provider 与外部业务的验收必须逐项读取台账。

## 3. 归属和不变量

- Pilot/Goose 决定业务方案、库和版本，编写与调试代码；Anchor 不增加 Python 开发专属 Agent loop。
- Host 应用用例拥有共享解释器/工具链的准备、配置及授权；Sandbox 执行文件、进程、网络和凭据边界。管理能力不能由候选清单自授权。
- Library 继续拥有唯一正式 Plugin、共享包存储与正式视图；Graph 只引用 Plugin ID，依赖不是第三类可发布资产。
- Session 拥有源码、测试、构建输出、每项目私有 venv 和候选/旧视图临时引用的归属。Host 保存环境选择、绑定与锁；开发资源不进入命名环境目录或每 Run 冻结环境模型。
- 同一个 Python 版本复用同一只读解释器；Pilot 可以用标准 pip 操作私有包，不能改写共享解释器的 site-packages。正式和候选包继续统一存储与来源校验。
- 正式运行仍从当前 Library 读取 Plugin，保存实际代码与依赖观察；沿用下一节点调用使用当前版本的既定语义。
- 本轮网络授权仍被冻结。需要修改授权或工具挂载时，Host 在当前执行收束后通过已有 Session/Turn 接纳机制接续，不要求用户手工刷新、重启或新建 Session，不另建恢复/计划系统。
- 正式安装保留现有原生确认、摘要核对和发布锁；开发实验不逐个变成安装审批。测试所需外部凭据必须有明确的候选授权，不能继承正式 Plugin 的凭据。

## 4. 实现方向与必须落实的契约

### 4.1 基础能力初始化

完整平台交付必须让缺少业务 Graph、Python、Rust 或业务 Plugin 时 Pilot 仍能工作。复用现有 `platform_status` 提供真实状态，新增少量 Host 用例准备和登记基础能力；uv/rustup 等工具负责安装细节，Anchor 负责目标目录、来源、校验、权限和归属。

配置应只有一个解析入口，每个有效字段只认一个权威来源。普通用户使用 Host 管理配置；显式部署配置保留外部管理的归属，不能被 Pilot 静默覆盖，也不能镜像到另一份 `.env` 后双写。动态配置、旧部署输入与只读策略的具体持久化契约须随实施说明并验证。

准备动作完成后，再发布可用事实；并发准备同一版本不得重复安装，取消或失败保留可核查结果。共享环境只在 Anchor 拥有且已授权的范围内管理，系统基础由交付入口处理。

联网入口及模型控制通道必须能在业务能力尚未安装时使用。联网授权、基础能力准备、插件启用等需要改变后续工具集合的动作，应有明确接续边界和幂等身份；不能因为复用同一 Goose Session 就修改正在执行的冻结权限。

### 4.2 准备并使用开发依赖

复用固定 uv、现有基础解释器和 Sandbox，为 Session 工作区中的项目创建私有 venv，并用共享解释器的 bundled `ensurepip` 引导私有 pip。参数绑定当前 owner/Session 及工作区相对项目，模型不指定任意宿主目录或直接挑选内部环境路径；只有环境子树挂载到稳定的 `/development/env`，选择事实与锁留在 Host。

正式依赖继续属于 `plugin.json`；pytest 等开发用依赖由项目文件和 pip 管理，不强迫正式插件携带开发工具。默认准备保留已安装试装包，源码或声明编辑不触发隐式安装、卸载或重建。显式 rebuild 只恢复空 venv/pip，依赖在后续命令安装；内联数组也只作显式安装。项目包私有、store 不挂进命令，共享解释器和正式/候选视图只读。

私有环境核对归属、布局与冻结解释器，不以准备收据代替包健康或测试观察。editable 源码修改无需重新安装第三方包；正式候选测试和安装仍分别核对被测试/批准的代码摘要，开发测试不自动覆盖修改后的交付内容。普通 pip 使用冻结任务网络和文件授权，正式取包沿原 Host 来源及哈希规则。

### 4.3 共享开发引用与生命周期

候选与旧项目视图引用 Library 共享包，保留 scope 归属与引用；私有环境位于该扫描范围之外。准备与执行持共享 scope lease，并持同项目排他锁至 Sandbox 子进程退出；不同项目独立，普通命令不持全局 Library 写锁。Session 删除持排他 lease、写 released 事实并清理私有环境；正式发布仍共用 Library 写锁。取消、部分失败或重启保留最小操作观察，由 Agent 核查后继续，不自动重放或增加审批。

回收的判据扩展为没有正式或有效开发视图引用，未知/损坏归属按保留处理。不能把 Session 准备目录直接挂到现有回收扫描却不登记引用，也不能依赖“进程已经挂载”来保护被删除的源目录。正式运行仍保留当前更新语义，不把开发引用扩展成历史 Plugin 快照。

这个变化影响依赖回收契约；实现时同步更新 [插件依赖标准](plugin-dependencies.md) 和真实受影响的测试，计划存在不等于当前标准或实现已改变。

### 4.4 自由开发的网络边界

允许联网的命令使用私有网络 namespace、`anchor-sandbox-network` 与 slirp4netns，禁用 host-loopback，不再使用 Bubblewrap `--share-net`。Host 的无 Key 管理入口只允许 loopback，非 loopback 必须配置 API key。沙箱代码不能通过业务出网绕过平台管理授权；网络进程启动失败不得回退共享宿主网络。

复用现有网络适配并确定可执行的管理端点隔离与身份校验方案，实际验证“可访问获准业务服务、不可借原始 HTTP 修改平台资源”。对需要访问本机业务服务的情形使用明确授权，不将所有本地服务等同于管理通道。资源限额与临时调试服务的生命周期按实际开发用例补齐，现有超时/清理不冒称完整配额已实现。

## 5. 垂直切片顺序

| 切片 | 交付结果 | 依赖及检查 |
| --- | --- | --- |
| V1 空业务安装中的共享 Python 初始化 | 模型配置后 Pilot 可发起联网授权、准备共享 Python/包来源，并在后续执行实际调用该解释器 | 固定 Goose/原生引导工具随交付准备；状态、发布/取消、并发与接续契约同时落地；隔离数据验收 |
| V2 普通命令使用开发依赖 | Pilot 准备空环境，标准 pip/项目文件安装、升级、卸载和 editable，读错误、改代码并继续 | 私有环境隔离、试装保留、显式重建、同项目锁、取消核查及旧视图兼容；两项目包版本隔离，共享 store 不变 |
| V3 正式交付闭环 | 按正式声明测试、原生确认安装、对话内启用，删除开发现场和 Session 后正式 Plugin 仍能调用 | 复用现有 review/prepare/test/install；保持正式与开发依赖区别及下一 Turn 生效语义 |
| V4 完整初始化边界 | 无预登记 Rust 的工具链准备，外部服务凭据/OAuth、渠道与计划按需求从 Pilot 配置和验证 | 复用已有 Runtime、OAuth、ChannelSupervisor、Scheduler；不同外部业务分别验收，缺凭据/失败如实保留 |

沙箱管理网络与权限边界是受影响切片的组成部分，不作为“后续再修”的前提缺口。分片顺序服务于交付可复用结果，不能以 V1–V3 完成删除 I1 的其他用户能力。

## 6. 首要验收场景

1. 在支持平台的干净安装/隔离根中，确认没有可借用的开发 Python/Rust 路径、包缓存、业务 Graph/Plugin 或上一台开发机配置；用户只配置模型。
2. 用户对 Pilot 提出一个需要第三方 Python 包的 Plugin 需求。Pilot 经对话内授权准备基础环境，实际命令输出已登记共享解释器的路径、版本及随机标记。
3. Pilot 选择第三方库及开发测试工具，运行普通脚本与 `python -m pytest` 等测试；至少实际经历一次代码修改与重测，保存真实工具结果和依赖身份。
4. 正式候选按运行依赖重新准备并进行 MCP 业务测试，经现有确认安装和对话内启用后实际调用；开发依赖未被隐式写入正式声明。
5. 删除开发现场与 Session，验证所属资源/开发引用释放，正式 Plugin 和依赖仍可调用。
6. 定向验证同版本并发准备、两项目依赖版本隔离、离线已有包/缺包、取消与重试、重启接续、回收不破坏有效视图及沙箱不能绕过管理接口。
7. 无预登记 Rust、凭据、渠道和计划分别取得后续真实行为证据后，才关闭 I1 的对应验收边界。

先做定向测试，跨模块切片完成后执行必要 Rust 全量回归与 Clippy/fmt；Web 按实际改变的对话授权与输入控件测试/build。Runtime 默认使用小型 Graph、真实 Host/Goose/Sandbox 和确定性模型传输；另做隔离真实 Provider 验收。没有真实 Provider、仅发现工具、只看到配置或跳过用例，都不能写成端到端完成。

## 7. 代码接入点与接续

| 接入点 | 职责 |
| --- | --- |
| `rust/anchor-runner-host/src/api.rs`、`main.rs` | 空业务平台启动、readiness、同一应用用例的 API 装配 |
| `pilot_tools.rs`、`pilot_host.rs`、`goose_acp/pilot_interaction.rs` | 状态/管理工具、冻结 Turn、原生交互与安全接续 |
| `tool_environment.rs`、`node_tools.rs` | 共享解释器、项目依赖上下文与普通命令装配 |
| `dependency_provision/{config,resolve,fetch,build}.rs` | 统一配置读取、成熟依赖准备与来源校验 |
| `application/plugin_candidates.rs`、`plugin_dependencies.rs`、`plugin_view.rs` | 候选准备/自测、共享字节、开发和正式视图 |
| `rust/anchor-library/src/reclaim.rs`、`pilot_resources.rs` | 开发引用保护、所属清理和回收 |
| `rust/anchor-sandbox-bwrap/src/lib.rs` | 命令的文件、网络、进程与资源边界 |
| `api/oauth.rs`、`channel_supervisor.rs`、`api/schedules.rs` | 现有业务配置能力的对话入口 |
| `rust/anchor-distribution`、`deploy/`、`apps/web/src/Pilot*.tsx` | 产品交付、基础输入与对话交互 |

接续先读 AGENTS.md、本文、开发台账最新相关记录，再核对当前架构与实际 diff。工作区有多条既有未提交改动，禁止 reset/checkout/clean 或自动提交无关路径。

接续入口以总台账最新的本专项记录为准；已实现的调用和持久契约以当前架构及上述接入表为准。普通实现选择依据仓库证据处理，实际改变持久事实、授权或恢复语义的决定在对应文档中明确记录。验收包记录二进制、固定 Goose 与归档摘要；测试时冻结独立副本，不把 Cargo 之后重建的产物或其他部署当作同一身份。

本文不要求重跑 A212 全部证据，也不把 A213 的服务部署误读为本专项完成。每个用户可见切片完成后才在总台账追加一次推进记录；测试、Provider 与部署结果分别记录。
