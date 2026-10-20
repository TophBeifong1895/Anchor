# M5 最终测量与交回

M1–M5 全量迁移的本轮工作已完成，按 §10.20 一次交回。任务书先单独提交为 **e60374f — docs: add M5 task book (§10.20)**；最终测量与收尾提交见本次交回。未推送、未删除旧 worktree/分支。后续问题保留在文末待决清单。

## 固定基点

迁移后固定为 master **e60374fd06eda07de8df9c41e64a5e03f6e3246d**；行数与测试旧基点为 **fc558e279eea9aaa68f4f26730285c3ee6f18515**；规则旧基点为 **1d1ba54035b2f25a45fd52c2536f5dc19cbd85e2**；C1 为 **085937332c6784e183abd5e6ee9522bbaf310730**。见 [baseline.json](baseline.json)。

静态统计只读固定 Git 对象，后续提交/工作区不进入测量。区间包含独立 master 工作，不将全部增删归因于迁移。ce90b5b→e60374f 的 Rust 差异只有独立提交 3282134：anchor-rsi/src/redact.rs +3/-3、tests/weekly.rs +12/-4，合计 **+15/-7**，无 Run 路径变化，因此本轮重跑 workspace。交回前无后续提交改到 Run 路径；本轮最终提交仅含文档与测量产物。

## 探针复核

未新增探针或生产源码。注册仍在 `rust/anchor-runner-host/src/application/run_access/kernel_runs.rs:99`；`kernel_probe.rs:26` 从 retained history 重建，`:60` 在线 best-effort 追加。Kernel history 是事实来源。

复用 `tests/routing.rs` 的 framed Host 确定性 Op 夹具形状，在独立目录 `/tmp/anchor-m5/probe-retained-ov2buh6q/state` 跑 schema 15 单节点 `true`。同一 Run 的投影分别被截断、删除后重开，两次都由现有 HostProbe 原生重建，resource/history 字节未变，Op 未重放。在线 **1** 点与独立按 history 派生的 **1** 点，九字段及顺序全等；缺失 **0**、额外 **0**、差异 **0**。原始文件副本在 `/tmp/anchor-m5/probe-snapshot`，不入库。可复跑 [probe_fixture.py](probe_fixture.py)、[probe_review.py](probe_review.py)，结果与身份见 [probe-fixture.json](probe-fixture.json)、[probe-summary.json](probe-summary.json)。

另独立复跑现有三个 Probe 测试，**3 / 0 / 0**：create/submit/control/parallel drive 真实样本、缺失/截断重建、I/O 失败不影响权威事实。负向用例的 I/O 诊断为预期。样本见 [probe-sample.jsonl](probe-sample.jsonl)，联合分组见 [probe-summary.json](probe-summary.json)；只提交测量投影，不提交完整 history 或原始日志。

| 样本维度 | 数量 |
| --- | --- |
| 总点 / 总 cost | 12 / 4 |
| 校验器 | validator/graph 12 |
| 身份 | Agent 5；Code 2；Human 5 |
| 动作 | Request 5；Write 7 |
| 裁决生效维度 | Accepted 7；Returned 2；Refused 3 |
| 裁决验证维度 | Evidenced 5；NotApplicable 7 |
| 原因码 | Ok 7；StaleVersion 2；TaskSettled 1；TaskWaiting 1；Unauthorized 1 |

样本来自本地确定性端口，未用 Goose 或模型 Provider。现有 Point 没有 Run id、seq、提交时间、模型和任务类别；跨 Run 重建只能比较多重集，不能还原全局时间顺序。已删 Run 的 history 不再可用，重建只覆盖保留的所有者。投影 I/O 丢点靠后续重建补齐；未做并发重建/崩溃规模压力试验。

复跑入口（原始输出只放 `/tmp/anchor-m5`）：

~~~sh
python3 experiments/constitution-host-preflight/final/measure_lines.py
python3 experiments/constitution-host-preflight/final/measure_c2.py
python3 experiments/constitution-host-preflight/final/test_inventory.py
python3 experiments/constitution-host-preflight/final/rules_audit.py
ANCHOR_KERNEL_PROBE_SAMPLE=/tmp/anchor-m5/probe-online.jsonl \
  /tmp/anchor-goose-regression-target/debug/deps/anchor_runner_host-2f70363a593aeab8 \
  application::run_access::kernel_runs::probe::tests --nocapture
python3 experiments/constitution-host-preflight/final/probe_fixture.py
python3 experiments/constitution-host-preflight/final/probe_review.py \
  /tmp/anchor-m5/probe-retained-ov2buh6q/state /tmp/anchor-m5/probe-online.jsonl
~~~

最后一项使用 probe-fixture.json 的 data_root，勿对正在写入的目录采样。二进制由本轮 workspace 构建；路径随 feature/构建变化时用 `cargo test -p anchor-runner-host --bin anchor-runner-host --no-run` 给出的实际产物。脚本需 Python 3.11+，Web 清点另需项目已安装的 TypeScript。

## 按 crate 的 Rust 行数

fc558e2→e60374f **物理差异行**，含注释/空行，不是 SLOC。不开 rename 侦测，移动算删除与新增。生产/测试按两版各自分类：tests/、bench、test_support、测试专用外部模块及内联 `#[cfg(test)]` / `#[cfg(all(test, ...))]` item 均计测试，含模块内 helper；未归类测试属性 **0**。共享 test-fixtures 按测试基础设施统计。examples/build.rs 计生产；Cargo 元数据、Web、文档、冻结数据不混入 Rust 行数。逐路径与方法见 [line-counts.json](line-counts.json)、[measure_lines.py](measure_lines.py)。

| crate | 生产删除 | 生产新增 | 测试删除 | 测试新增 |
| --- | ---: | ---: | ---: | ---: |
| anchor-devtools | 117 | 129 | 140 | 417 |
| anchor-distribution | 160 | 108 | 43 | 278 |
| anchor-docmost-tools | 1 | 0 | 1 | 0 |
| anchor-file-lease | 0 | 53 | 0 | 66 |
| anchor-graph-host | 270 | 631 | 225 | 1,158 |
| anchor-kernel | 0 | 1,482 | 0 | 1,524 |
| anchor-library | 16 | 24 | 0 | 0 |
| anchor-mcp-host | 6 | 53 | 0 | 0 |
| anchor-network-tools | 0 | 0 | 0 | 0 |
| anchor-platform-session | 84 | 422 | 14 | 1,579 |
| anchor-rsi | 60 | 441 | 1 | 1,014 |
| anchor-runner-host | 3,646 | 7,621 | 2,199 | 14,392 |
| anchor-runtime | 2,094 | 1,960 | 4,405 | 8,964 |
| anchor-sandbox-bwrap | 0 | 0 | 0 | 0 |
| anchor-scholarly | 0 | 0 | 0 | 0 |
| anchor-wecom-gateway | 5 | 6 | 15 | 76 |
| anchor-wecom-tools | 0 | 0 | 0 | 0 |
| **合计** | **6,459** | **12,930** | **7,043** | **29,468** |

生产净增 **6,471**，测试净增 **22,425**；Rust 合计删除 **13,502**、新增 **42,398**，净增 **28,896**。Kernel 的 3,006 行是在比较区间内加入的内核及测试，相对已批准 0859373 仍零 diff。增加涵盖 Kernel、Host 门面/观察/恢复端口及独立 master 功能，测试扩充占大头，不能据此推断 Host 变小或全部归因于迁移。Runtime 生产净减 134，Host 生产净增 3,975；规则收敛另按下节审计。

## 八类规则与 C2

两版每个独立判断点的 `文件:行` 见 [rules-audit.md](rules-audit.md)；[rules_audit.py](rules_audit.py) 校验位置，[rules-counts.json](rules-counts.json) 保存 107 个锚点、35 个源码 blob。复用同一视图/派生函数的消费者不重计，不将不同事实的分支加成“生命周期总数”。

| §10.1 规则 | 基点重核结论（旧→后） |
| --- | --- |
| 是否终态 | 生命周期谓词 1→1；JSON 未结束识别 1→1，旧 JSON 漏项本次补齐。 |
| stop/resume/abandon/pause | stop 4→1、pause 5→1；resume 建议 2→1、worker 门禁 3→1，旧终态/未知/额外预算写前门禁分别 1→0；核查、子绑定、drive 选择另列。abandon 建议 0→1、命令幂等门禁 2→1、旧最终写保护 1→0。 |
| 是否静止 | Host 2→1；Web 独立显示公式不是恢复/删除授权。 |
| 是否关注 | Host 2→1；Web 显示另列。 |
| 迟到结果 | Host 普通 Work 裁决 0→0，由 Kernel 统一裁决；活跃子回调绑定 1→1，另保留旧分支 0→1，静态绑定合计 1→2，与控制/释放规则去重。 |
| 未知效果 | 旧恢复目标 2→1、Runner 异常直接写失败 1→0；外部事实/传输/防重放边缘保留，新增 native attempt/人工说明/history 投影另列，不能用旧 13 与新窄表 4 拼接总下降。 |
| 预算耗尽 | 普通 resume 额外预算门禁 1→0、子 BudgetStopped 允许重入过滤 1→0；端口预算观察唯一映射在 Runtime，W4 未裁定。 |
| 子 Run 结算释放父 Run | Call 结果投影 1→1，活跃唤醒前 delivery/绑定核对 2→1，保留旧分支额外 0→1，静态合计 2→2；实际 Code resume 在共用 Runtime scheduler。 |

当前没有 `Some` 生产调用的 `control_expected(..., waiting)` 绑定分支仍保留，已按静态独立判断点计入额外 1 处；没有以不可达为由豁免。旧 detached 扫描漏项与 Host/Web 显示差别都已登记，M5 未改源码。

C2：H1 **save 16、mutation 36 → 0 / 0**。[M3 52 项去向](../m3/c2-destinations.md) 逐项重核，当前定位用函数名：KernelRuns::create/control/recover_attempt/execute_standalone、CallRunAccess::admit。M4 后 GraphRunRecord schema 15 删除 error/recovery/recovery_submissions/updated，`reads.rs:146` 只向临时 RunView 填历史投影，没有保存 Graph record。

[measure_c2.py](measure_c2.py)、[c2-counts.json](c2-counts.json) 记录固定树扫描：生产 Host/GraphHost 的 save 仅余 settings 和 native conversation owner 的 3 处，逐项排除；无 GraphRunRecord mutable 参数或候选字段 mutation。词法扫描仅为定位证据，0/0 同时依赖 52 项去向及 M4 字段归属复核。

## 测试与登记

| 源码口径 | fc558e2 | e60374f |
| --- | ---: | ---: |
| Rust workspace 测试函数，含 ignored | 1,821 | 2,162 |
| 普通 / ignored | 1,706 / 115 | 2,036 / 126 |
| Goose ignored 子集 / 其他 ignored | 111 / 4 | 122 / 4 |
| Web 单测 callback，each 计一个定义 | 213 | 280 |
| Web 浏览器 callback | 84 | 99 |

[test-counts.json](test-counts.json) 给出 helper、cfg、参数及 ignored 原因。普通 helper 不计，带测试属性的 subprocess helper 每侧 3 个计入。源码包含 cfg 分支，不代表当前平台全执行；doctest 不计函数，Web 参数/循环不展开。六个共享 fixture 函数被当前 28 个 target 引用，加 Runtime 2 个 doctest，解释了源码 2,162 与 workspace 执行 2,326（2,200+126）的差异；Host 完整 ignored 123 也不等于 Goose 源码函数数。

[test-mapping-summary.md](test-mapping-summary.md) 汇总 M1–M4 条数与链接，保留原单位：M1 Rust 26 / Web 4 处；M1.5 Rust 21 / Web fixture 1；M2 合并 11 组及后续夹具登记；M3 13 类与同步适配；M4 9 类。Runtime 原测试 73 项、Host/GraphHost 消失函数名 39 项、M4 改名/等价替换 34 项各有去向，不相加为互不重叠删除数；重复 Kernel Goose target 1 个退出，12 个业务场景由默认 target 保留。有效 S **8**（S19–S24、S26、S27）均批准；S25 作废，S27 结清 S4。M5 新 S、断言修改、删除测试、新 ignore 均 **0**，业务断言未放宽。

## 手写判断与宪法复核

逐条位置及不能从 Kernel 定律推出的原因见 [规则审计](rules-audit.md#迁移后仍手写的边缘判断)。Session 保留 Turn/inbound/current assistant、前驱继承、投递/budget hold；授权保留冻结身份/digest、Plugin/Library 与收件范围；执行端口保留 worker 排空/锁、pause/abandon 命令、native tool facts/attempt、取消/deadline 和 child binding。Kernel 不拥有这些事实，不能从 Run phase 猜测。

| “完美”检验 | 有证据的结论 |
| --- | --- |
| 组件由宪法推出或属于可替换边缘 | Run 经提交裁决；GraphValidator 持 Graph 规则；Session/授权/端口各拥有事实；Host/API/Web/Probe 为适配或投影。未新增 Agent loop、恢复引擎或第二份 Run writer。 |
| 本迁移是否需改核心 | 已批准存储切片含在 0859373，之后 Kernel 零 diff。M5 无源码改动，不增 Reason/Waiter/Phase/Store/Stamp。 |
| 核心是否可删 | 未发现有证据可删的责任；零 diff 不能证明绝对最小。M4 指定的四个 CompletionFact 都有端口用途，见 [字段表](../m4/field-disposition.md)。 |

四极限只能给结构依据，不能宣称实证证明：

| 宪法 §7 极限 | 结构复核 | 未验 |
| --- | --- | --- |
| 智能无限 | 身份/成本由宿主盖章，提议/裁决分离，模型替换不进入 Kernel | 更强真实模型、完整真实 Provider 业务验收 |
| 规模无限 | 按所有者分区；点从各自 history 派生，不建全局事实日志 | 大 Run/历史、多 Host/并发压测；启动全 history 重建仍线性 I/O |
| 时间无限 | accepted history/端口观察恢复事实；未知先核查，不自动重放 | 长期运行、介质耗尽、保留策略、长周期崩溃矩阵 |
| 存在对手 | 权限只下降；身份门禁/Graph 校验/沙箱边界；固定 Goose 有拒绝与不重放证据 | 对抗压力、渠道实网、最终部署联合验收 |

W4 明确未结：端口预算耗尽目前等待人，宪法默认要求结算；Kernel 账本硬上限与端口 BudgetExhausted 是不同边界。本轮不替用户裁定，不能声称宪法全部目标已实证满足。时间/模型/任务类别等测量坐标未补齐。

## 本轮验证与 §9 交回

命令、退出码、原样 test result 摘要、SHA256 与日志路径见 [checks.json](checks.json)。原始日志只存 `/tmp/anchor-m5`；M4 原日志在 `/tmp/anchor-m4`，按已有证据引用。[verify.py](verify.py) 开始要求 Rust 与基点一致，验证前后比较源码摘要，本轮一致。脚本编译/结构校验及措辞检查通过。

| §9 项 | 结果 |
| --- | --- |
| 里程碑 / commits | M5；e60374f 独立任务书，最终测量/收尾提交见本次交回。 |
| workspace | **132 targets，2,200 / 0 / 126**，本轮新跑，因基点含 RSI 差异；数量与 M4 相同。 |
| Web test / build | 本轮未重跑；Web 源码对 M4 无 diff，引用 M4 **359** 与 build 通过。浏览器本轮未验。 |
| C2 | save **0**，mutation **0**，52 项去向成立。 |
| 删除 / 新增 | 固定区间 Rust **13,502 / 42,398**；生产 **6,459 / 12,930**，测试 **7,043 / 29,468**。含独立 master 工作，非 M5 源码改动量。 |
| 对照 | M5 未重跑；M3/M4 的 30,000 例冻结回归保留，快照无改动。 |
| S / 断言 / 删除 | 本轮均 **0**；历史按上文不同单位汇总。 |
| 真实 Goose | 本轮不重跑完整固定 Goose，按 §10.20 引用 M4 **41 targets，123 / 0 / 0**，七身份一致；本轮 Probe 3 项通过。真实模型完整业务未验。 |
| fmt / Clippy / 围栏 | 本轮 fmt、围栏 **1 / 0 / 0** 通过；严格 Clippy 只引用 M4，本轮门槛不要求重跑。 |
| C1 | `git diff 0859373 e60374f -- rust/anchor-kernel` 空；M5 无 Kernel 改动。 |
| 停止条件 | 未触发；无源码/裁决规则变化，未放宽业务断言。 |
| 待决定 | 见下表，本轮不替用户裁定。 |

## 迁移后待决清单

| 事项 | 当前事实 / 未关闭边界 | 决定者 |
| --- | --- | --- |
| 生产数据离线迁移 | **用户已决定不提供**，Anchor 尚未上生产。旧 schema/RunStore 具名拒绝，不自动迁移；M5 未删开发数据 | 用户已决定，登记；未来如出现需求另立任务 |
| W4：预算耗尽是否结算失败 | 端口 BudgetExhausted 为 waiting/human/reason=budget，Kernel 硬预算定律不变 | 用户决定语义 |
| 真实 Provider、外部渠道实网、部署验收 | 确定性 Goose 已验；独立真实模型小图仅证明各自场景，不证明完整业务/实网/最终部署，M5 未补做 | 用户/审查方定范围，部署维护者提供环境与执行 |
| 时间是否成为 Kernel 事实 | Stamp/Point 无提交时间，Host updated 为观察；模型/任务类别坐标也未补齐 | 用户决定事实边界与优先级，核心变更另审 |
| history / points.jsonl 保留策略 | 删除 Run 删除 owner history；重建只覆盖保留 Run，在线旧点可在下次重建消失。无 TTL/归档政策，长期成本未验 | 用户/数据维护者决定政策 |
| constitution-core worktree/分支何时删除 | 现工作在 /home/mansteinl/Anchor master，旧 worktree/分支保留；未删除、未推送 | 用户决定时机 |
| 审计边缘差异 | dormant waiting 分支、Host/Web 静止显示差异已登记，不影响 C2；显示不作操作授权 | 后续维护者按产品需求决定是否另开整理任务 |

迁移任务随 M5 交回结束，以上保留为独立后续范围。
