# P：串行 Kernel 路径本地验收

日期：2026-10-17。基线 `a234cb6`；实现提交 `ff09f2c feat(host): kernel-path probe for serial Runs (P)`。结论：**任务书要求的七类本地场景、1000 seed 对照和测量交付完成**。默认旧路径继续通过；没有切换生产默认入口或删除旧 Runner。测量、外推假设及未决边界见 [measurement.md](measurement.md)。

## 场景证据

应用层测试位于 `rust/anchor-runner-host/src/application/kernel_runs/tests.rs`，通过 RunApplication 所属 KernelRuns、Kernel<FileStore> 与既有 NodeExecutionPort / ArtifactPort；控制和放弃用例调用现有应用入口。七类场景没有只在独立调度器上验证。

| 任务书场景 | 测试名 | 实际覆盖与结果 |
| --- | --- | --- |
| 1. 串行完成 | `serial_probe_commits_artifacts_cost_and_phase_through_application` | 3节点，3个Agent结果提交、产物证据、总成本9，Done；查询/列表投影；没有旧Run记录。通过 |
| 2. 执行中stop/resume | `stop_refuses_late_result_then_resume_uses_durable_completion` | 先接受Human stop，再观察取消；迟到结果TaskWaiting拒绝并留历史；resume重用持久完成事实，a#1不重执行。通过 |
| 3. 等待中abandon | `abandon_waiting_probe_is_terminal_and_refuses_late_submission` | 应用abandon结算Aborted，之后Settle得到TaskSettled。通过 |
| 4. 重启恢复 | `restart_recovers_same_invocation_from_completed_or_not_started_fact` | 第一节点已提交，第二节点fixture报错；drop应用与FileStore后重建实例，分别覆盖第二节点Completed / NotStarted事实，同invocation恢复，第一节点不重执行。未模拟OS强杀。通过 |
| 5. 未知效果 | `uncertain_effect_parks_until_human_verification_then_resume_or_abandon` | Uncertain携Observation停在等待人，无execute；fixture模拟核查后写入完成事实再resume，另测abandon。未接真实Goose核查。通过 |
| 6. 节点失败 | `waiting_recovery_outcome_parks_and_known_failure_settles` | WaitingRecovery停在等待人；另一个Run的已知Failed由Code Settle失败结算。通过 |
| 7. 轻量差分 | `application_probe_matches_old_runner_on_one_thousand_serial_seeds` | 复用原random_graph、Scripted、run_old和facts，seed0..999，原3%失败哈希；逐一比较派发顺序、输入commit、全部Graph事实及最终相位/原因。通过 |
| 额外：硬约束 | `probe_guards_keep_one_writer_and_charge_stale_work_once` | 同目录第二writer拒绝；StaleVersion重试只收费一次；内容自述999999不改变宿主cost=3；Agent Settle Unauthorized；双存储权威读取Conflict。通过 |
| 额外：现有真实Host路径 | `api::tests::kernel_probe::explicit_kernel_probe_runs_real_host_ops_and_projects_existing_queries` | POST /trigger显式kernel_probe，实际PreparedExecution/node_host/Bubblewrap/Artifact跑3个Op.run，生成report.txt，GET详情/列表投影完成，旧RunStore为空，终态stop返回409。通过 |

上述9项新增测试均出现在 [host.log](host.log)。API用例没有经过Goose，不能据此宣称Agent/provider端到端已验。

## 最终源码检查

以下命令均实际执行，工作目录为仓库 `rust/`。退出码与耗时见 [checks.json](checks.json)，本节只记录最终源码的验收运行，不把此前修复过程算作验收。

| 命令 | 结果 | 原始输出 |
| --- | --- | --- |
| `cargo build -p anchor-sandbox-bwrap --bins` | 退出0 | [build.log](build.log) |
| `cargo fmt --all -- --check` | 退出0，无输出 | [fmt.log](fmt.log) |
| `cargo clippy -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host --all-targets -- -D warnings` | 退出0 | [clippy.log](clippy.log) |
| `cargo test -p anchor-kernel` | 37通过，0失败，0忽略 | [kernel.log](kernel.log) |
| `cargo test -p anchor-runtime -- --nocapture differential` | 5个差分测试通过；串行10000、并行失败5000、原并行覆盖5000、调用5000及495/2122独立场景 | [differential.log](differential.log) |
| `cargo test -p anchor-runtime` | 127通过，0失败，0忽略；另2 doctest通过 | [runtime.log](runtime.log) |
| `cargo test -p anchor-runner-host -p anchor-graph-host --no-fail-fast` | runner-host **1083通过/0失败/112忽略**；graph-host **46通过/0失败/0忽略**；合计1129通过/112忽略，较基线增加9 | [host.log](host.log)、[各target结果](host-results.json) |

graph-host的46包含42项lib测试和4项child_lifecycle；没有把这4项算入runner-host。112项忽略为既有项目，本刀未新增ignore，也未将其记为端到端通过。差分完整Tally在原始日志，关键门槛仍为串行node_failed=772、并行node_failed=775/multi_failure_waves=6/ceiling_preempted_failures=2、原无失败parallel_activations=5048；失败组parallel_activations=4775单列。共享fixture提取未修改这些概率、范围、门槛或断言。

## 约束与未覆盖

文档新增的27个本地链接全部可获取；既有台账的18个 `.local` 证据入口在此工作区不可获取，未用作本刀验收证据，详见[链接检查](document-links.json)。

静态结果见 [constraints.json](constraints.json)：`git diff 6ee2b0e -- rust/anchor-kernel` 为空；新scheduler与KernelRuns生命周期文件中 `stop_requested|pause_requested` 零命中；无新增ignore。生产GraphValidator仅提取既有资源ID格式为共享函数，裁决未改；请求构造/授权逻辑和测试fixture为原实现提取共用，没有另造节点授权或Agent loop。

成本通过宿主submit参数；NodeSubmission中的model_requests不被Kernel作为计费来源，额外测试用自述999999与宿主3证明边界。身份由固定Authority限制，Agent不能Settle。Kernel FileStore保存资源及裁决历史，旧RunStore不保存探针Run；返回的GraphRunRecord.status/error只是Phase投影副本。目录单writer与每Run worker登记负责宿主执行排他。

**真实Goose：未执行。真实Provider：未执行。** 本环境 `ANCHOR_GOOSE_BINARY` 未配置且PATH没有goose（只检查是否存在，未输出凭据），故依任务书使用fixture端口；本轮也未运行固定Goose确定性回归。Web、部署、旧Run迁移、并行/Call/常驻助手探针、旧Goose attempt恢复与删除未验。resume对于真实Uncertain不会消除未知；需要执行后端先给出核查事实，否则继续等待。Unsupported及查询兼容耦合已登记于测量文档，不冒充生产级迁移完成。
