# 设计卡 005：让人以外的触发也能借机器人发消息

> **状态：已实施（2026-10-26），S1–S5 完成，§8 逐条取证。未做真实企微端到端投递。**
>
> 流程：设计卡 → 用户批准 → 按卡施工 → 偏离设计时停下改卡再问 → 对照 §8 验收清单逐条取证。
>
> 来源：用户 2026-10-26 的讨论。用户已同意的三点方向：
> - 授权方式是在绑定上显式列出「允许代发的 Graph」，不是「挂了插件就能发」；
> - 收件人只限绑定里已有的 `send_users`，不能 `@all`，不能发群；
> - 发送形态是确定性的 Host 操作节点，回执作为产物，不是给任意 Agent 节点挂工具。
>
> 本卡只解决「非人手触发的 Run 如何借一个已启用的机器人发出消息」。见微日报、播客、音频分片上传、邮件兜底都在 §9 里列为后续方向，这次不做。

## 1. 现状（读码结论，行号以 dd2fda4 为准）

`ba575da` 之后，定时任务不再固定以 `"local"` 身份运行，而是传原始安装 owner。新装的 `local` 安装上，定时触发与手动触发因此得到同一个凭据身份；老的 `api:` 安装上两者仍不同（定时任务没有经过 `binding_owner`，由独立的修复 PR 处理）。但即使凭据身份一致，**定时或后台 Run 依然发不出消息**，原因有两层，而且两层是独立的：

### 1.1 工具只挂在助手的 work 节点上

`channel_tools.rs:51` 的 `wrap` 在 `channel_tools.rs:64-66` 有一个早退：

```rust
if metadata
    .assistant
    .as_ref()
    .is_none_or(|assistant| assistant.work_node != key.node_id)
{
    return inner;
}
```

`metadata.assistant` 只在渠道助手 Run 中有值。定时任务跑的是普通 Graph，这个字段是 `None`，因此在第一行就返回，`wecom_send_message` 和 `wecom_attach_image` 都不会出现。

这是 `3362bd9`（2026-10-25，设计卡 004 S3）有意收紧的：它删掉了原先「只要节点挂了 wecom 插件就注入工具」的宽松判断，改为只认 R1 的助手形态。设计卡 004 §9 也明确把「后台 Run 结果回到对话」列为后续方向。

### 1.2 Host 操作节点只认助手

`node_host/assistant.rs:471` 的兜底分支返回 `host operation is not authorized for this node`，而 `node_host/assistant.rs:260-267` 的 `execute_host_operation` 第一步就要 `assistant::source(...)` 非空，否则报 `host Session operations require an authorized assistant binding`。`execution.rs:90-103` 在 Run 准入阶段也拦一道：只要 Graph 里有任何 `host` op，而 Run 没有 assistant 元数据，就直接拒绝创建。

**所以「新增一个 Host 操作节点」这条路，在当前上游并不存在可以复用的入口。** 照 `session.reply` 的形状加一个 `channel.send` 操作行不通：那套 host 通路整条都绑定在助手身份上。

### 1.3 顺带确认的事实

- `AssistantPlan::from_snapshot`（`assistant.rs:52`）要求一个助手**恰好**是 `wait_input → Agent → reply` 三节点，因此周报这种六节点 Graph 不能被当成机器人，也无法通过「给周报加一个 wait_input」来绕过；那会让它变成助手形态并改变其语义。
- 绑定里已经有 `send_users`（`management.rs:59`），`allowed_recipient`（`channel_tools.rs:196`）已经在拒 `@all`，并且只接受名单内成员或 `*`。这部分可以直接复用，不需要新机制。
- 网关的控制面（`gateway/control.rs:27`）目前只有 `status` 和 `send` 两个操作，`send` 只支持 `userid + content` 的 markdown 文本。**没有任何发文件的能力**，音频要从这里过就必须先扩协议。
- 网关的出站媒体上传管线（`connection.rs:463-521`）已经是三阶段的 `aibot_upload_media_init/_chunk/_finish` 再 `aibot_respond_msg`，但它是按「回调回复」组织的：`job.callback_id` 是回复的锚点，`media_step` 的 `Sending` 阶段用 `aibot_respond_msg` + `callback_id` 发。**它不能直接用来做主动推送**，因为主动推送没有回调可回。

### 1.4 线上表现

- 在含 `3362bd9` 的版本上，依赖「挂了 wecom 插件就有发送工具」的周报通知会静默消失：节点不会报错，工具只是不存在，Agent 会如实说「没有这个工具」。
- 周报的 Graph 本身也从来没有通知节点：`examples/graphs/weekly-work-report.json` 的节点是 `collect / understand / write / review / gate / publish`，`publish` 是 `run` 操作，跑的是 `rsi weekly-publish` 本地命令。

## 2. 目标

1. 一个已启用的机器人绑定，可以显式授权**指定的** Graph 在**非助手** Run 里向它自己的 `send_users` 发文本消息。
2. 发送是确定性的：给定同样的输入和同样的绑定，投递结果和回执可复现、可测试；不接受「Agent 自己决定发不发」这种形态。
3. 未授权的 Graph、未启用的绑定、名单外的收件人、`@all`、群发，全部在产生任何副作用之前拒绝。
4. 现有助手形态（R1）行为完全不变，`wecom_send_message` 仍然只在助手的 work 节点上出现。
5. 不新增运行时阶段、不新增持久事实类型、不新增一个平行的发送实现。

## 3. 定义

- **代发授权**：绑定里的一份 Graph 名单，声明「这些 Graph 的 Run 可以借这个机器人发消息」。它是绑定的一部分，随绑定一起保存、一起停用、一起删除。
- **代发 Run**：一个非助手 Run，其 Graph 名字出现在某个已启用绑定的代发授权名单里。
- **发送请求**：`{ graph, userid, content }` 三元组。`graph` 决定用哪个绑定，`userid` 必须在那个绑定的 `send_users` 内。

**规则：**

- **R5 授权跟着绑定走，不跟着 Plugin 走。** 挂了 `wecom` 插件不等于能代发；必须是绑定里点名的 Graph。这条是 §1.1 那个收紧的延续，不是回退。
- **R6 收件人上限取交集。** 实际可用收件人是「绑定的 `send_users`」而不是「绑定的 `users`」。`users` 是能对机器人说话的人，`send_users` 是能被机器人主动打扰的人，两者语义不同，不能合并。
- **R7 代发只发文本。** 图片复用现有助手通路；文件、音频、视频不在本卡范围内（见 §9）。
- **R8 代发不创建会话、不产生 Turn、不写渠道消息账本。** 它是一次性的出站投递，没有对话上下文。回执写入发起 Run 的产物，不进 Session。
- **R9 一个 Run 只能用自己 Graph 名下的绑定。** 不能借别的 Graph 的机器人。Graph 名由 Host 从 Run 记录里取，不接受节点参数里传。

## 4. 方案

### 4.1 机制选型：为什么不是 Host 操作节点

§1.2 已经说明 host op 通路整条绑在助手身份上。要在那里开口子，等于把「host op 必须有助手」这条准入不变量拆掉，影响面覆盖 `execution.rs`、`assistant.rs`、`api/tests/persistent_assistants.rs` 的多个用例，而且会让任何 Graph 都能声明 host op——这正是设计卡 004 花力气收紧的东西。

**改用 `Op.run` 命令节点。** 理由：

- `Op.run` 已经是非助手 Graph 的常规形态，周报的 `publish` 就是它，不需要动准入。
- 它的产物是工作区里的文件，天然符合「回执作为产物」。
- 它不需要新的命令白名单：`Op.run` 只要求沙箱里有 `sh`，能不能发消息取决于 Host 有没有挂载 `/channel/<bot>/`（§4.3.1），授权点在 Host 一侧，显式、可审计。
- 失败会作为节点失败暴露，不会静默。

代价是它会起一个子进程。这个代价可以接受：`publish` 已经在起子进程，周报的 Graph 不缺这个能力。

### 4.2 命令形状

新增一个 Host 内置命令 `anchor-channel-send`。它就是 Host 二进制本身，只读挂载在沙箱的 `/tools/anchor-runtime/anchor-channel-send`，按 `argv[0]` 分发，和 `anchor-route` 一样由 `add_route_helper` 放进 PATH。

```
anchor-channel-send --bot <bot-graph> --userid <userid>
```

- `content` 从 stdin 读，不放进命令行参数（避免超长参数和 shell 转义问题）。
- `--bot` 是**要借用的那个机器人所属的 Graph**，不是当前 Run 的 Graph。这两个名字必然不同：代发的定义就是「A 的 Run 借 B 的机器人」，写成一个参数就会自相矛盾。
- 当前 Run 的 Graph 名由 Host 从 Run 记录里取，节点无法声明，也无法覆盖。
- 成功时向 stdout 输出一行 JSON 回执，退出码 0；被拒绝时向 stderr 写明原因，退出码非 0。
- 回执里**不含** token、descriptor 绝对路径、BotID、Secret。

命令的代码在 Host 二进制里，但作为沙箱内的子进程运行；它和助手的 `channel_tools` 共用 `channel_control.rs` 这一个控制客户端（§4.3），不另写一份。

### 4.3 复用而不是新写

现有的投递链路（`channel_tools.rs:346` 的 `send` → `control_endpoint()` → Unix socket → 网关 `gateway/control.rs` 的 `Send` 分支）已经具备从 descriptor 读 socket 路径和 token、20 秒超时、未确认不重发、网关侧按 `request_id` 幂等。代发不另写一份，而是把这条链路里与入口无关的部分抽成共享客户端 `channel_control.rs`，助手的发送工具和 `anchor-channel-send` 都只经过它：

- **限额**：正文 ≤20480 字节、`userid` ≤200 字符、整个请求帧 ≤64KB。三个数与网关一致，由测试 `limits_match_the_gateway` 钉住；Host 不为此依赖网关 crate。超限在连接 socket 之前就拒绝。
- **请求帧**：同一个函数生成 `{"operation":"send", request_id, userid, content, token}` 加换行。
- **确认规则**：最多等 20 秒回执；只有 `accepted: true` 且回显**同一个** `request_id` 才算送达。网关的真实回执总会回显它（重复请求另带 `duplicate: true`）。回执缺失、格式错误或属于别的请求，一律算「不确定」，不自动重发。

两个入口只在下面两处不同：

| 项 | 助手 | 代发 |
| --- | --- | --- |
| 绑定怎么找 | `metadata.graph` 的 `saved_binding` | 由 Host 在挂载阶段决定，见 §4.3.1 |
| `request_id` 怎么算 | `channel_request_id(key)`，含 Session 与 tool_call 身份 | 对 `(run_id, node_id, invocation, bot, userid)` 做 SHA-256：前缀 `anchor-channel-delegated-send-v1\0`，各字段带长度前缀，结果为 `channel-delegated-<hex>` |

同一 Run 的同一节点重试时 invocation 不变，得到同一个 id，网关只投递一次；不同 Run 的 id 必然不同。正文**不**参与计算：周报恰好和上周措辞一样时，新 Run 仍会送达，不会被当成重复吞掉。同一个 id 若带着不同的正文再来，网关按冲突拒绝（`send request_id was already used for different content or identity`），不会发出第二条。

#### 4.3.1 授权点在挂载，不在参数（本卡最重要的修正）

子进程能改自己的环境和命令行，所以「命令自己声明它是哪个 Graph」不能作为授权依据。真正的授权必须在 Host 一侧、在起子进程之前完成。

`Op.run` 的沙箱只挂载 `/usr` `/bin` `/lib` 等系统路径（`anchor-sandbox-bwrap/src/lib.rs:37`），**宿主的数据目录在沙箱里根本不存在**。因此：

- 助手通路的 `control_endpoint()` 那种「自己读 descriptor」在 `Op.run` 里做不到；
- 反过来，这也说明「挂上 descriptor」本身就是一次能力授予，天然是授权点。

做法：`Op.run` 启动前，Host 读 Run 记录得到当前 Graph `G`，向渠道管理要「有效启用且 `delegate_graphs` 含 `G`」的全部绑定，把每个绑定的 `control.json` 和 `control.sock` 只读挂进沙箱：

```
/channel/<bot-graph>/control.json
/channel/<bot-graph>/control.sock
```

没被授权的 Graph 一个挂载都拿不到，命令即使被调用也只能报 `no bot is delegated to this Graph`。node 用 `--bot` 在已挂载的集合里选一个，选不到就拒绝。

`/channel` 需要加进 `main.rs` 的 `authorize_readonly_destination_root`，与 `/in` `/plugins` `/spill` 并列。

**代价要写清楚**：挂载后沙箱里能读到该 bot 的 control token，也就能绕开本命令直接向 socket 发 NDJSON。这是「这个 Graph 被授权用这个机器人」这条授权的自然含义，不是本卡引入的新漏洞；收件人上限仍由网关自己强制（`gateway/control.rs` 的 `allowed(users, userid)`，这里的 `users` 就是 Supervisor 传给网关的 `send_users`），`@all` 在那一层就过不去。

### 4.4 授权名单的存储与界面

`ChannelFact`（`management.rs:54`）新增一个字段：

```rust
/// Graphs allowed to send through this bot outside their own assistant Run.
#[serde(default)]
delegate_graphs: Vec<String>,
```

- `#[serde(default)]` 让现有的 format 2 绑定文件继续可读，不需要版本号跳变。**这是本卡唯一的存储改动**，不新增文件、不新增目录。
- 校验：元素必须满足 `identifier::is_name`，去重，最多 32 个，不能包含自己（助手 Run 不需要代发）。
- 改这个字段和改 `users`/`send_users` 一样，**必须先停用机器人**。理由和设计卡 004 一致：正在跑的网关不应该在脚下换授权。
- WebUI 加在现有的机器人配置面板里（`apps/web/src/GraphBot.tsx`），一个多行输入或多选列表，跟成员名单并排。保存走现有的 `PUT /graphs/{graph}/channel`，`ChannelBinding` 加同名字段。
- `ChannelMembers` 也要加一个 `delegate_graphs: Option<Vec<String>>`，但语义是**「省略即保持不变」**而不是「省略即清空」。原因：同一个端点也服务 Pilot 的成员表单（`pilot_tools/channel_management.rs`），那张表单没有这一项；如果省略等于清空，操作员在 Pilot 里改一次收件人就会静默撤销 WebUI 里刚配好的代发授权。
- Pilot **不能**设置这个字段（传 `None`）。代发是操作员授权，不是模型可以给自己的权限。Pilot 的 `channel_configure` 只接受 `graph`、`users`、`send_users` 三个参数，显式传 `delegate_graphs` 会被拒绝。
- 启用时的校验：名单里每个 Graph 必须存在且能加载。不满足就拒绝启用，说明是哪一个。

### 4.5 拒绝路径要具体

每一条拒绝都在产生副作用之前发生，并且给人看得懂的原因：

| 情况 | 谁拒绝 | 原因（stderr 原文） |
| --- | --- | --- |
| 不在 Graph 节点里运行（缺 Run 身份环境变量） | 命令 | `anchor-channel-send must run as a Graph node` |
| `--bot` 不是合法 Graph 名 | 命令 | `"<bot>" is not a Graph name` |
| 当前 Graph 未被授权、绑定未启用、网关没在运行，或 `--bot` 写错 | 命令 | `no bot is delegated to this Graph under /channel/<bot>` |
| 正文为空 | 命令 | `content is empty` |
| 挂载目录在，但 descriptor 读不到 | 命令 | `this Run was not granted that bot` |
| 正文超过 20480 字节，或 `userid` 超过 200 字符 | 命令（连接之前） | `message or identifier exceeds platform limits` |
| 网关 socket 连不上 | 命令 | `gateway is not reachable` |
| `userid` 不在 `send_users`，或是 `@all` | 网关 | `recipient is not allowed` |
| 网关没连上企业微信 | 网关 | `gateway is not authenticated; no message was dispatched` |
| 回执缺失、格式错误或属于别的请求 | 命令 | `channel delivery was not confirmed; do not resend automatically` |

「未被授权」和「`--bot` 写错」在命令看来是同一件事：`/channel/<bot>` 不存在。能力在挂载阶段就没给，命令只是把这个事实说清楚；Graph 里写错 `--bot` 是最常见的失误，所以原因里带上它查找的路径。最后一行不是拒绝而是「不确定」：消息可能已经发出，节点失败，但不能自动重发。

### 4.6 明确不改变的东西

- 助手 Run 的工具注入逻辑一行不改。
- `ChannelFact` 的其它字段、format 号、文件名、凭据位置都不动。
- 网关的控制面协议不动（本卡只发文本，`Send` 已经够用）。
- 不新增 API 路由；`delegate_graphs` 跟着已有的 `PUT /graphs/{graph}/channel` 走。
- 不新增持久事实类型；回执是普通的工作区文件产物。

## 5. Web 与 Pilot

- **Web**：`GraphBot.tsx` 的机器人配置面板加「允许代发的 Graph」一项。已启用时只读并提示先停用，和成员名单的现有交互一致。
- **Pilot**：不改。Pilot 工具的入参形状不动，`delegate_graphs` 不是 Pilot 能改的东西（它属于操作员授权）。
- **Graph 删除**：不额外处理。删除一个被授权的 Graph 后，它的名字留在别人的 `delegate_graphs` 里，效果只是「一个不存在的 Graph 被授权」，没有任何 Run 能匹配上。启用时的存在性校验（§4.4）会把它报出来，让操作员顺手清掉。改绑定文件需要持有 supervisor 锁、且要求先停用，为了删 Graph 去做这些不值得。

## 6. 迁移与部署

**不需要迁移。** `#[serde(default)]` 让旧的 format 2 绑定读出来是空名单，行为等价于「谁都不能代发」，正好是当前的实际行为。

部署顺序：

1. 先上二进制（含本卡改动），此时所有绑定的 `delegate_graphs` 都是空的，行为与现在完全一致。
2. 在 WebUI 里给需要代发的绑定填上 Graph 名单并启用。
3. 把周报 Graph 的 `publish` 之后接一个 `notify` 节点，调 `anchor-channel-send`。

第 3 步是使用方的事，不是本卡的交付物；但本卡要在文档里给出可直接抄的例子。

## 7. 需要你拍的决定

> **已按推荐项实施（2026-10-26）：D10-A、D11-A、D12-A。**

- **D10｜代发授权的粒度**
  - **A（建议）按 Graph 名。** 名单里写 Graph 名，一个 Graph 的全部 Run 都能代发。
  - B 按 Graph + 节点名。更细，但每个节点的名字一改就要维护名单，而且周报的节点名会变。
- **D11｜授权名单能不能为空字符串表示「全部」**
  - **A（建议）不能。** 空就是谁都不行，必须显式列名字。`*` 在这个字段里不接受。
  - B 接受 `*` 表示所有 Graph。方便，但等于把这台机器的每个 Graph 都授权了，和 §2 的目标 3 冲突。
- **D12｜代发失败时节点的行为**
  - **A（建议）节点失败，Run 停在失败状态，回执里带原因。** 理由：代发是确定性动作，失败就该暴露，和 `publish` 一致。
  - B 只记警告、Run 继续成功。理由：通知不是主体交付物。但这会让「周报发出去了吗」变成不可知。

## 8. 验收清单

| 编号 | 用例 | 断言 |
| --- | --- | --- |
| T20 | 授权的 Graph 用 `anchor-channel-send` 发给 `send_users` 里的人 | 网关收到一次 `send`，回执 JSON 有 `accepted: true` 和 `request_id` |
| T21 | 未授权的 Graph 的 `Op.run` | 沙箱里没有 `/channel` 下任何挂载；命令报 `no bot is delegated to this Graph` |
| T22 | 授权 Graph 发给 `send_users` 以外的人 | 网关拒绝；命令非 0 退出，回执不含 `accepted` |
| T23 | 授权 Graph 发给 `@all` | 网关拒绝；命令非 0 退出，回执不含 `accepted` |
| T24 | 绑定被停用后同一个 Run 再跑 | 该 Run 的挂载集合里没有它；命令拒绝 |
| T25 | `--bot` 指向未挂载的机器人 | 非 0 退出，原因含 `no bot is delegated` |
| T26 | 同一 Run 的同一节点重试两次 | 两次算出同一个 `request_id`；网关只投递一次 |
| T27 | 助手 Run 的 `Op.run` 节点调用同一命令 | 助手 Run 同样按 Graph 授权，不享受特权；未授权时拒绝 |
| T28 | 助手 Run 的 `wecom_send_message` | 行为不变（回归 card 004 的 T11） |
| T29 | 启用时 `delegate_graphs` 里有一个不存在的 Graph | 拒绝启用，说明是哪一个 |
| T30 | 绑定处于启用状态时修改 `delegate_graphs` | 拒绝，提示先停用 |
| T31 | 旧 format 2 绑定文件（没有该字段） | 正常读取，代发名单为空 |
| T32 | Pilot 的成员表单改收件人，绑定里已有代发名单 | 代发名单保持不变，不被清空 |
| T33 | 回执内容 | 不含 token、不含 descriptor 绝对路径、不含 BotID/Secret |
| T34 | 周报 Graph 的 `notify` 节点样例 | 在 fixture Graph 上跑通，文档里的例子与之一致 |
| T35 | 绑定指向自己的 Graph | 拒绝保存，原因说明机器人自己的助手 Run 已经能发 |
| T36 | 网关回执的 `request_id` 属于别的请求 | 不算送达：非 0 退出，原因为 `channel delivery was not confirmed`，stdout 没有回执；助手发送工具同样处理 |

## 9. 明确不做（后续方向）

- **音频、文件、视频出站。** 网关控制面只有 `send`（文本），出站媒体管线绑在回调上（§1.3）。要做需要：给 `control.rs` 加一个 `send_file` 操作、给 `connection.rs` 抽出一条不依赖 `callback_id` 的主动上传路径、处理 64KB 控制帧上限（分片走 `aibot_upload_media_chunk` 已经在用 512KB 分片，但控制面本身是 NDJSON 单帧 64KB，所以分片必须由网关自己从磁盘读，不能从 Host 传 base64 过来）。这是一张独立的卡。
- **邮件兜底。** 需要 SMTP 配置、附件大小限制、失败重试，是另一套投递通道。
- **发群。** 群需要 `chatid`，而当前 `allowed_recipient` 是发给 `userid` 的。群的授权模型和成员名单不是一回事。
- **代发图片。** 助手通路已经能做，但那条路要求 `assistant::binding` 存在（`channel_tools.rs:207` 的 `reply_path`），代发 Run 没有 Turn binding。要做得先设计代发 Run 的图片暂存位。
- **让任意 Agent 节点自己决定发消息。** 这是 §1.1 那个收紧明确否掉的方向，本卡不重新打开。
- **见微日报与播客的完整迁移。** 依赖上面的音频出站。

## 10. 施工顺序

每一步都要能独立通过测试。

1. **S1 存储与校验**：`ChannelFact` 加 `delegate_graphs`，加 `#[serde(default)]`，加 `ChannelBinding` 字段，加校验和「启用时不能改」的规则。此时没有发送能力，只有配置。
2. **S2 授权查询**：一个只读函数，给定 Graph 名回答「它能借哪些绑定代发」。单元测试覆盖空名单、未启用、Graph 不存在。
3. **S3 命令与投递**：`anchor-channel-send`，与助手发送工具共用 `channel_control` 一套限额与确认规则，网关回执须回显同一 `request_id` 才算送达；接上 §4.5 的全部拒绝路径。这是主体。
4. **S4 Web**：`GraphBot.tsx` 的配置项，含已启用时只读。
5. **S5 文档与样例**：`docs/wecom-assistant.md` 补一节，`examples/graphs/` 加一个带 `notify` 的周报变体，`docs/pilot-development-plan.md` 更新当前验收条目。

**没有部署步骤。** 本卡不改变任何现有行为，部署与否不影响线上；等 §6 的第 2、3 步由使用方决定。

## 11. Deployment verification (2026-10-28)

One operator-approved deployment exercised the real private-message path: a
non-assistant Graph's Op invoked `anchor-channel-send` through an explicitly
delegated bot. The returned request id matched a gateway `send/confirmed` ledger
entry, and the notification receipt gate passed. Existing channel Sessions
recovered; the original member lists and model settings were retained. No model
call or full report publication was needed for this test. This verifies platform
acknowledgement, not recipient reading, the next scheduled full report, group
sends, media delivery or simultaneous use of multiple real bot accounts.
