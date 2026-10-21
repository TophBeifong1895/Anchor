# P7 单次盲测结论

2026-10-21。实现了 [信号 Graph](../../../examples/graphs/rsi-signals.json) 与 [只读 stdio Plugin](../../../examples/rust-rsi/plugins/harness-hub/README.md)，完成一次真实默认 Flash 盲测。**内容命中 2/3，验收失败。** 独立评审要求修订；原始 Run 在 gate 启动解释器时失败，未发布，也没有进入第二轮模型执行。

## 三处缺陷的判分

命中要求同时指出工具和具体失败条件。宽泛的“路径被拒”不计命中；单次现场复现只能证明充分条件，不能证明 A 中每次失败都由它引起。

| 缺陷 | 盲测原文与证据 | 判定 |
| --- | --- | --- |
| `anchor_run` 携带 `project`，非 Pilot AgentNode 不可用 | investigate-1/4 找到无条件公开的 schema 与 `development_projects=None` 拒绝分支；用 `project:"."` 的无副作用命令复现；综合稿 M1、提议 P1 明确写出条件 | **命中** |
| `anchor_edit` 拒绝 `/workspace/...` | investigate-2 从 `{content,path}` 形状排除 sha/匹配次数假设，提出 workspace 前缀归一化；综合稿 §0.2 实际尝试 `/workspace/synthesis.md`，保留完整拒绝文案，P3 提出接受 workspace 前缀 | **命中**；历史 28 次的具体路径值仍未知 |
| `anchor_read` 拒绝 `/workspace/...` | investigate-3 定位绝对路径必须属于只读输入挂载的分支，却把来源推测成 hub/Host 的 `/local-inputs/...`；综合稿 M3/I3/P2 沿用该解释，没有指出 workspace 输出同样被挡 | **未命中**；定位了相关代码，未找回要求的触发条件 |

评审原文不会替代上述内容判分。它确实识别了综合稿中的证据越界、层级混用和预测判据问题，结论为 `REVISE`，不是合格发布稿。

## 预测与已知 B 组对照

以下 B 组结果仅在盲测结束后用于评分，未提供给被测 Agent。B 同时修了三个工具，并非逐条实施本轮提议，故只能比较方向和数值范围，**不能据此确认提议的因果效果**。

| 提议 | 盲测预测 | P6 实际 B | 评价 |
| --- | --- | --- | --- |
| P1 / run | `not_executed` 从 30/854 降至 ≤5；validation 又使用 ≤8；token 降 2%–4% | 相关失败 30→1 | 失败方向与数量上限相容，两个验收阈值不一致；时间/token 收益未证实 |
| P2 / read | `not_authorized` 从 13/188 降至 ≤5，总成功率 ≥0.97 | 相关失败 13→0 | 数值方向相容，但未命中 workspace 条件；提议还改了错误文案，存在“换标签即达标”的通路 |
| P3 / edit | 含前缀归一化时 `invalid_arguments` ≤7/169；对应形状失败 0 | 相关失败 28→0 | 方向相容，预测保守；一条提议里把 level 0/1/3 混为条件值，应拆开 |
| P1–P3 / 质量 | `non_decreasing` | 全对 A 93/100、B 97/100；无 `contrast_quality_loss` | “不降”的总体方向相符，不能证明每任务 ±0.02 等更细条件 |

整体工具失败率 A 6.1%（1,310 次调用）→ B 0.8%（1,246 次）。本轮没有给出一个可直接评分的整体失败率预测。P6 用时比 0.979（p=0.20）、token 比 0.930（p=0.12），均未显著；不能把本轮 2%–4% token 预测写成已验证。P4–P9 的重试度量、评分口径、题面和实验设计提议未实施，也没有下一轮数据判定，G5 未通过。完整 [9 条原始提议](proposals.json) 保留原文，**未按评审意见代改**。

## 运行身份与盲法

- 隔离 Host 和给 Agent 的 Host 源码固定在 `1af26b1273acba4fc422e0e2723798dd0d87afa5`。源码副本只含该提交的 `rust/`，没有 `.git`、后续提交、docs、experiments 或工作区本地目录。
- 中枢使用当前只读二进制与源码；explore 另得 `anchor-rsi` 源码对照。唯一数据根是 A：`.local/harness-hub-p6/exp1/A`，只读挂载；未挂载 B、P6 结果文件、任务书 §P6/§P7 或本交接书。
- 节点用 `models.default.flash`，实际模型为 `deepseek-flash`，网络业务工具关闭。真实执行的是 Host → GraphRunner → Goose ACP → 授权工具/MCP，未用模型替身。
- 模型试验只有一次。此前 `blind-01` 是 0.63 秒的 MCP 启动路径错误，模型请求为零；修正 Plugin 二进制装配后，`blind-02` 才是本次试验。Run ID 仍为 `p7-blind-01`，目录名不表示第二次模型试验。
- 盲测使用的 [冻结 Graph](trial-graph.json) 是评分对象，运行 Graph digest 为 `401030df60437636493695f297ecd78bbc3033bc5de167d1124ef89016e1842f`。结束后只为可复用版本修正 Python 启动；没有改调查说明来重跑答案。

原始 Run 终态 `settled/failed`，原因是 `sh: python3: command not found`。本机 `/usr/bin/python3` 经 `/etc/alternatives` 间接链接，沙箱未挂载该目标；可用的 `python3.12` 没被 gate 选择。现版先试可执行的 `python3`，再试 `python3.12`，运行器也显式授权后者。**原始盲测不因此补签成功。**

保存的提议与评审已通过真实旧 Host 的 Op-only 回放：gate exit 0，路由 `triage`；替代 triage 的确定性终止节点主动退出，阻止模型重试。另有沙箱内 PASS fixture 验证可进入 publish 路由。没有做修正后真实模型的完整发布或修订循环验收。

## 成本与纪律

| 项目 | 本轮 |
| --- | ---: |
| 墙钟 | 941.899 秒 |
| Agent invocation / 模型请求 | 9 / 319 |
| input / output tokens | 19,647,996 / 301,175 |
| total tokens（含重复上下文） | **19,949,171** |
| cache-read tokens（包含在 input 中） | 19,002,240 |
| Provider 价格估算 | $0.334584；非账单结算 |
| 工具调用 / 失败 / 丢弃观察 | 358 / 16 / 0 |
| `records` / `invocation` / `invocations` 调用 | 12 / 15 / 10 |

| 节点 | total tokens | 工具调用 |
| --- | ---: | ---: |
| signals | 361,499 | 16 |
| triage | 523,235 | 22 |
| investigate-1 | 1,865,420 | 41 |
| investigate-2 | 2,561,590 | 42 |
| investigate-3 | 1,256,608 | 42 |
| investigate-4 | 2,894,052 | 52 |
| explore | 7,231,441 | 94 |
| synthesize | 2,135,225 | 26 |
| review | 1,120,101 | 23 |

`records=12` **不是全部原始读取量**：explore 有 46 次 shell 命令引用数据挂载，含目录检查、直接读事实/grade/产物及遍历 100 个 Run 的 meta 文件。它绕过中枢，说明当前装配没有强制“运行数据只能经 MCP 读”。不能用这个代理指标宣称上下文受 k 严格限制。没有同输入的旧 `rsi` 模型运行，G4 的新旧成本与提议质量比较仍未验证。

核对全部 358 条工具观察及 shell 命令：未发现写 Host/Graph/Plugin、扩大权限或执行 level 3 提议；写操作在本节点 workspace，包括产物、形状计算脚本与临时探针。A 数据与 Host 源码运行前后文件哈希一致。观察保留 4 次并发 `stale_version` 返回，最终研究分支均提交，不把它们删去。导出前按当前真实凭据字节检查，命中文件数 0，临时模型配置已移除。

**注意力纪律未完全满足**：四支均读取自己的 signal-N，未发现读取其他槽位内容；但 investigate-2/4 又调用了完整 `signals`，调查报告也引用其他工具桶。Host 传递全部上游 Artifact，MCP 不按信号限权；“拿不到别的信号”没有实现，不能把提示语当围栏。

## Graph 编排的问题与未关闭项

1. **triage 夹带根因猜测。** 14 条选了 run/edit/read 三个失败桶和 run 的 `call_corrected`，覆盖三类工具，但给 signal-N 增加了 hypothesis/test/falsifier 等额外字段，还提出 sha/白名单猜测。调查花时间推翻上游假设，run 的两个槽位高度重叠。应让 triage 输出原始信号与简短选择理由，把假设留给调查。
2. **读路径调查过早收敛。** 代码里已经有绝对路径检查，却只围绕“输入挂载之外”与诊断分类展开，未把正在使用的 `/workspace` 写输出场景作为反例。综合阶段也未补做 read 的 workspace 探针。该缺陷不是没选信号，而是选中后没有穷举最小路径类别。
3. **成本没有被编排约束住。** signals/triage 为搬运 JSON 也消耗大量模型轮次；explore 达 7.23M tokens 并直接读原始目录。说明缩短提示不等于减少读取。本版说明每角色 283–616 字符，旧 RSI 为 1,238–2,409；篇幅缩短已经做到，成本收益尚未做到。
4. **数据与信号访问边界没有强制隔离。** 后续应先明确中枢进程专属数据挂载与按信号输入契约，再谈严格 O(k)。不为这次试验另写一套通用 Runtime 或改变 Kernel。
5. **文件交接依赖模型自行修复。** 调查/探索产物多数未用声明的 investigation-N.md / explore.md 名字；综合通过目录列举找到了实际文件。应显式要求固定输出名，并做确定性文件存在检查。
6. **反馈不是修改稿闭环。** 当前 `REVISE` 回 triage，再跑研究与综合；triage 只有 gate/旧 triage 的声明，没有确定性的提议修订通道，可能重复昂贵调查。实际修订轮未执行，不宣称恢复闭环通过。
7. **评审有效但 gate 只能校验结构。** REVISE 抓到了分类变更伪装 level 0、双阈值、改标签降低失败数、口径改变当质量改善等问题。字段齐全不足以证明提议正确；P8/P9 即使获评审局部保留，也没有自动执行或写入提议账本。

## 证据入口

- [机器判分与身份/哈希索引](score.json)、[成本汇总](metrics.json)、[完整提议原文](proposals.json)、[验证命令与边界](verification.md)。
- [全部节点产物](../../../.local/harness-hub-p7/collected/artifacts/)，含四支调查、探索、综合、评审；[综合稿](../../../.local/harness-hub-p7/collected/artifacts/synthesize/1/synthesis.md)、[评审稿](../../../.local/harness-hub-p7/collected/artifacts/review/1/review.md)。
- [完整工具观察 JSONL](../../../.local/harness-hub-p7/collected/tool-calls.jsonl)、[完整原始轨迹压缩包](../../../.local/harness-hub-p7/collected/raw-trace.tar.gz)，后者含 Goose 原生 SQLite 会话及 WAL、ACP 事件/用量/工具观察、Kernel 历史、Artifact 与冻结 bundle；不是只有摘要的指针文件。
- [运行终态](../../../.local/harness-hub-p7/blind-02/response.json)、[输入与源码逐文件清单](../../../.local/harness-hub-p7/blind-02/source-manifest.json)、[前后哈希核对](../../../.local/harness-hub-p7/blind-02/meta.json)、[无模型回放](../../../.local/harness-hub-p7/gate-replay-03/)。

原始轨迹保留在本工作区 `.local/`，不入 Git；约 6.5 MiB，SHA-256 `1209a6afea12b2b42783d284f7294b713932f9c37e467cb14127967aa1ff60e5`。共享报告到其他机器时需一起复制该包及其索引。仓库内提议、Graph 快照和评分可独立审阅；没有伪造缺失的发布产物。
