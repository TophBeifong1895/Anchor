# M4 §10.19 交回

状态：**M4 实现及 §10.19 全部门槛完成，交回审查**。完整固定 Goose 最终为 41 targets、123 / 0 / 0，七个二进制采证前后身份一致。M5 未开工，不推送。

任务书先独立提交为 **2cef07b — docs: add M4 task book (§10.19)**。实现从该提交开始，在 /home/mansteinl/Anchor 的 master 工作目录完成；期间独立业务提交 a98757a 涉及 Session 助手读取及对应文档，7b7fa59 只更新企微部署说明，两者不属于 M4 成果。最终实现与证据提交号由交回消息提供。

## 结果

| 门槛 | 实际结果 |
| --- | --- |
| workspace | 132 targets，**2,200 / 0 / 126**，退出 0。相对 M3 的 2,197，多出的 3 项属于独立 Session 提交 a98757a；M4 未增加 Rust 测试或 ignore。 |
| Web test / build | **43 文件、359 通过**；构建退出 0。相对 M3 增加失败原因/恢复展示的 3 项业务回归；Vite 既有大 chunk 提示保留。 |
| Runtime 定向 | Graph 140 / 0 / 0；完整 workspace 随后通过。 |
| Host 门面定向 | 80 / 0 / 0；恢复在线视图与只读磁盘投影一致。 |
| 完整固定 Goose | **41 targets、123 / 0 / 0**，退出 0；最终第二轮七个产物摘要一致。首轮亦为 123 / 0 / 0，但预先冻结顺序不正确，单独保留。 |
| Clippy / fmt / include 文件格式 | workspace/all-targets 严格 Clippy、cargo fmt check，以及 migrated include 文件 rustfmt check 均退出 0。 |
| 围栏 | 1 / 0 / 0；定义及计数清单均无变更。 |
| C1 / C2 | Kernel 相对 0859373 零 diff；save 0、mutation 0。 |
| Graph 裁决 / 快照 | validator、logic 及 30,000 例冻结数据相对 2cef07b 零 diff；旧执行器及轮询名称扫描零命中。 |

逐 target 原样 test result、命令、退出码、原始日志路径及 SHA256 在 [checks.json](checks.json)。原始日志仅存本机 /tmp/anchor-m4，不提交 .log；定向原始日志另有 /tmp/anchor-m4-runtime.log 和 /tmp/anchor-m4-host-targeted.log。

准备阶段删除字段后出现过编译失败，修复剩余引用后定向与完整 workspace 均通过。首次直接调用 rustfmt 因 PATH 不包含该工具退出 127，改用绝对 rustup 后发现并修正 include 文件格式，最终检查退出 0。不能把准备阶段写成首次全绿。

## 归属及覆盖

[字段处置表](field-disposition.md) 对 GraphRunRecord 每个字段逐项登记。error/recovery/recovery_submissions/updated 及 touch 删除；恢复事实只由已接受 Kernel history 投影，在线和只读入口共用同一纯函数。失败原因来自 Kernel Outcome::Failed，HTTP 保留 state.error 和 state.recovery；Web 仍显示失败原文与待恢复提示。Graph schema 15，旧 1–14 和旧字段按 C6 具名拒绝，不升级或重写旧数据。

sequence 保留为 Graph 结果/边决定的逻辑时钟；format/Wire 保留为 Graph schema 边界，不能由通用 Resource 生命周期版本替代。Plugin、Call、并行、invocation/pass/module、结果与边等 Graph 事实保留。CompletionFact 四个指定变体 **无可删**，逐个当前生产使用位置见字段表。

[断言登记](assertion-mapping.md) 共 **9 类**；[机器映射](test-mapping.json) 包含 **34 个函数改名或等价替换**（包含 subprocess helper）。删除测试 **0**，既有测试新增 ignore **0**，业务断言放宽 **0**，冻结快照修改 **0**。新增 Web 三项分别验证时间线失败原文、详情失败原因及渠道未知效果的原因/动作约束。

## 规则与宪法

[规则审计](rules-audit.md) 覆盖终态、控制、静止/关注、迟到结果、未知效果、预算与父子释放；独立判断体没有增加。C2 原 [52 项去向](../m3/c2-destinations.md) 继续有效；本轮删除读取后向 Graph record 回填恢复容器的赋值。结构命令及结果见 [structure-checks.json](structure-checks.json)。

新增组件属于 Graph 事实或可替换 Host 查询边缘，核心不需修改；未发现有证据可删的 Kernel 责任。四个极限逐条结构复核见规则审计，不据此声称规模压测、长期运行或真实模型验收完成。S 候选 **0**；不新增 Reason/Waiter/Phase/Code 提交种类，不改变 Graph 裁决与 Session 规则，§8 停止条件未触发。

## 采证与边界

使用 master 标准构建目录 **/tmp/anchor-goose-regression-target**。最终完整命令及全部环境逐项保存在 checks.json 的 goose_full 中，原始输出 /tmp/anchor-m4/goose-final.log，fixture 证据 /tmp/anchor-m4/goose-final-evidence；首轮日志 /tmp/anchor-m4/goose-first.log 和 fixture 证据 /tmp/anchor-m4/goose-full-evidence 保留。workspace/Clippy 及 helper/example build 后，最后按正式单包测试范围预编译，再冻结 Host、relay、sandbox-network、wecom-tools、docmost-tools、http_fixture 与 Goose 七个文件。Goose 固定摘要为 17e065a79cb0508413af714724aed51d6b76972dda9feebdda701330134fb640。首轮预编译后又构建了 helper，特性集合切换使 Host/relay 的清单与正式测试实际使用的缓存产物不同。首个 ACP fixture 的真实 Host 摘要与测试结束后的文件一致；首轮结果保留，但不充当最终身份验收。第二轮先完成全部 helper，再按最终测试范围预编译并重新冻结七个摘要，完整回归结束后七个摘要全部一致，见 [binary-identities.json](binary-identities.json)。构建顺序要求已同步标准回归文档。

验证源码以 [source-identities.json](source-identities.json) 逐路径 SHA256 和 Git blob 绑定，M4 的 29 个源码文件在采证期间未变。期间独立 RSI 两文件及周报文档修改未纳入 M4，普通 workspace 数字不代表这些后续修改已经过本轮验证；共享验收文档只提交 M4 行。

固定 Goose 使用确定性本地 Provider，实际经过 Host、ACP、授权 MCP、Sandbox 和 Artifact；真实模型 Provider、外部渠道实网、部署未验，W4 未裁定。本任务没有执行开发数据清理；并行业务文档中的授权重置不属于本轮验收。旧 Run 迁移工具按用户此前决定不提供。resource 与 history 跨次读取的一致性是既有边界，本轮未改变并发/恢复契约。

## §9 交回摘要

| 项 | 结论 |
| --- | --- |
| 里程碑 / 提交 | M4；任务书 2cef07b，最终实现及证据提交见交回消息。 |
| 测试 / Goose | workspace 2,200 / 0 / 126，Web 359 及构建；固定 Goose 123 / 0 / 0，七个身份一致。 |
| C2 | save 0，mutation 0。 |
| 删除 / 新增行数 | 源码删除 **255**、新增 **341**；Runtime +199/-222、Host +77/-33、Web 测试 +65/-0；[逐路径统计](line-counts.json)。不含文档、证据和独立 Session 修改。 |
| S / 断言 / 测试 | 新 S 0，断言 9 类，34 个函数映射，删除测试 0；业务约束无放宽。 |
| C1 / fmt / clippy | 全部通过，Kernel 零 diff。 |
| 停止条件 / 待决 | 无新增触发或新增架构裁定；M4 全部门槛完成，待审查，M5 未开始，不推送。 |
