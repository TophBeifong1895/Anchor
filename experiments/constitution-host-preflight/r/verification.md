# R0 / R 本地验证

日期：2026-10-17；分支 `constitution-core`；基线 `1d1ba54`。R0 `cb2219c fix: release every file lease explicitly before close (R0)`；R `54e664e feat(host): probe phase-based Run reads and Host observations (R)`。R0已同步master `6394e28`。**本地测试通过，测量结论为“需要重想”；不是全量迁移通过。**

## 最终验证

实际工作目录为仓库的 `rust/`；下列命令依次执行一次。完整参数、退出码和耗时见[checks.json](checks.json)，原始输出各自保存；fmt日志为空表示无输出成功。

```bash
cargo build -p anchor-sandbox-bwrap --bins
cargo fmt --all -- --check
cargo clippy -p anchor-file-lease -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host -p anchor-library --all-targets -- -D warnings
cargo test -p anchor-file-lease -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host -p anchor-library --no-fail-fast
```

四条exit code均0。原始输出：[build.log](build.log)、[fmt.log](fmt.log)、[clippy.log](clippy.log)、[tests.log](tests.log)。逐target汇总为[test-results.json](test-results.json)。

| crate | 通过 | 失败 | 原有忽略 | 相对四crate基线 |
| --- | ---: | ---: | ---: | --- |
| anchor-kernel | 37 | 0 | 0 | 不变 |
| anchor-runtime | 127 + 2 doctest | 0 | 0 | 不变；原差分测试含串行10000、并行5000、Call5000 seed |
| anchor-runner-host | 1087 | 0 | 112 | 比1083新增4项；没有增删ignore |
| anchor-graph-host | 46 | 0 | 0 | 不变 |
| 四crate小计 | **1299** | **0** | **112** | 基线1295 + R新增4 |
| anchor-library | 74 | 0 | 0 | R0迁移锁后验证；R未改此crate |
| anchor-file-lease | 3 | 0 | 0 | R0新增 |
| 本次全部 | **1376** | **0** | **112** | 58个有结果摘要的target，含doc-tests |

R0另曾执行以下命令，覆盖本次最终命令未重复选取的devtools/gateway；R不改这些路径，因此没有再跑其全量：

```bash
cargo test --manifest-path rust/Cargo.toml -p anchor-file-lease -p anchor-library -p anchor-runtime -p anchor-devtools -p anchor-wecom-gateway -p anchor-runner-host -p anchor-graph-host --no-fail-fast
cargo clippy --manifest-path rust/Cargo.toml -p anchor-file-lease -p anchor-kernel -p anchor-runtime -p anchor-runner-host -p anchor-graph-host -p anchor-library -p anchor-devtools -p anchor-wecom-gateway --all-targets -- -D warnings
```

均成功，原始输出[r0-tests.log](r0-tests.log)、[r0-clippy.log](r0-clippy.log)。这次测试选取中不含Kernel；总计1505通过/0失败/112原有忽略，其中devtools92、gateway78、Library74、Runtime129（含2 doctest）、runner-host1083、graph-host46、FileLease3。Kernel37由R最终验证实际执行。

## 新增用例与定向验证

R新增4项，原有断言全部保留：

- `run_view::tests::actions_cover_every_phase_waiter_and_outcome_with_and_without_worker`：Executing、三种Waiter、五种Outcome分别带有/无worker，共18组动作结果。
- `api::tests::kernel_probe::kernel_read_view_http_tracks_running_human_wait_and_settlement_with_host_time`：实际HTTP GET/stop/abandon，观察运行中、等待人及已结算的phase/actions；排空前resume不可用，排空后可用。宿主时间哨兵与Kernel存储mtime解耦，接受stop后时间改变，迟到及终态拒绝不改变时间。执行端口为本地Gate，未使用Goose。
- `application::kernel_runs::tests::broken_host_observation_never_blocks_kernel_control_or_lifecycle_reads`：故意损坏独立观察目录；stop/abandon仍被接受，Phase和动作可读，updated保持未知。日志中的观察失败诊断为预期。
- `run_projection::tests::legacy_projection_serialization_preserves_ready_paused_and_stopped_bytes`：旧Ready/Paused/Stopped三组投影JSON精确字节比较，补充原有全部状态动作/静止测试；没有删除或放宽旧断言，不声称穷举所有HTTP响应字节。

定向阶段最后执行的命令（仓库根目录），四条均通过，之后才执行上述最终全量：

```bash
cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --bin anchor-runner-host kernel_read_view_http -- --nocapture
cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --bin anchor-runner-host broken_host_observation -- --nocapture
cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --bin anchor-runner-host run_projection::tests -- --nocapture
cargo test --manifest-path rust/Cargo.toml -p anchor-runner-host --bin anchor-runner-host run_view::tests -- --nocapture
```

输出：[http-test.log](http-test.log)、[observation-test.log](observation-test.log)、[projection-test.log](projection-test.log)、[view-test.log](view-test.log)。定向开发中曾修复新HTTP用例的stop成功码（应202）与abandon reason（应operator），以及测试编译错误；没有改动既有API规则来迎合新测试。这里保留最终输出，未把先前失败写成成功。

R0新增3项锁测试，覆盖持有复制描述符时的提前返回、panic展开，以及共享/独占和失败争用不释放他人锁。22个生产Rust获取站点的原策略都保留；另有沙箱Cargo缓存初始化的嵌入shell flock，锁由subshell持有，按独立生命周期保留并登记在lock-sites.json。未新增ignore或改变旧断言。

## master同步

任务书要求的R0已同步到`/home/mansteinl/Anchor`：`6394e28 fix: release every file lease explicitly before close (R0)`。只同步文件租约修复；保留master的Runtime shared read lease和credential lease原可见性，没有带入该分支的Kernel/P/H1行为。master tracked工作区已核对为空，原有untracked研究文件未处理。

master相关all-target编译检查通过（runner-host、Library、devtools、gateway及依赖）；FileLease3 + Runtime122单测/7集成测试共132通过，0失败/忽略。原始输出[r0-master-check.log](r0-master-check.log)、[r0-master-tests.log](r0-master-tests.log)；这些日志未包含启动命令行，本报告不补写无法从保留输出核实的完整参数。没有在master运行全量Host/provider验收，没有部署或推送。

## 结构与兼容性

以下实际检查的diff均为空，命令与结果见[invariants.json](invariants.json)：

```bash
git diff 6ee2b0e -- rust/anchor-kernel
git diff 1d1ba54 -- rust/anchor-runtime/src/graph/validator.rs rust/anchor-runtime/src/graph/validator
git diff 1d1ba54 -- apps/web
```

C1通过；没有改生产GraphValidator裁决或差分门槛。一个Run仍只归Kernel FileStore或旧FileRunStore之一，同ID双存储拒绝；Host时间观察没有被任何控制/授权/恢复判断读取为依据。原生RunView路径通过；旧消费者仍依赖临时适配读取原record，严格的单一RunView输入未验收，不能记为统一迁移完成。旧Run的API字段、值与控制分支保留；原生Run的P字符串/动作兼容收在JSON边界，返回副本不落盘。全局C2、旧Runner删除等不在本刀验收范围。

任务书要求的原始RunStatus搜索如下，完整内容亦见[runstatus-grep.txt](runstatus-grep.txt)：

```bash
grep -n "RunStatus" rust/anchor-runner-host/src/application/{run_projection,liveness,conversations,kernel_runs}.rs
```

```text
rust/anchor-runner-host/src/application/run_projection.rs:129:    use anchor_runtime::graph::RunStatus;
rust/anchor-runner-host/src/application/run_projection.rs:149:            (RunStatus::Ready, true, true, false, false),
rust/anchor-runner-host/src/application/run_projection.rs:150:            (RunStatus::Running, true, true, true, false),
rust/anchor-runner-host/src/application/run_projection.rs:151:            (RunStatus::Paused, true, true, false, false),
rust/anchor-runner-host/src/application/run_projection.rs:152:            (RunStatus::WaitingCall, true, true, false, false),
rust/anchor-runner-host/src/application/run_projection.rs:153:            (RunStatus::WaitingRecovery, false, true, true, false),
rust/anchor-runner-host/src/application/run_projection.rs:154:            (RunStatus::BudgetStopped, false, true, true, false),
rust/anchor-runner-host/src/application/run_projection.rs:155:            (RunStatus::Stopped, true, true, false, true),
rust/anchor-runner-host/src/application/run_projection.rs:156:            (RunStatus::Completed, false, false, false, true),
rust/anchor-runner-host/src/application/run_projection.rs:157:            (RunStatus::Failed, false, false, true, true),
rust/anchor-runner-host/src/application/run_projection.rs:158:            (RunStatus::Aborted, false, false, true, true),
rust/anchor-runner-host/src/application/run_projection.rs:173:        record.status = RunStatus::Running;
rust/anchor-runner-host/src/application/run_projection.rs:192:                RunStatus::Ready,
rust/anchor-runner-host/src/application/run_projection.rs:196:                RunStatus::Paused,
rust/anchor-runner-host/src/application/run_projection.rs:200:                RunStatus::Stopped,
rust/anchor-runner-host/src/application/liveness.rs:28:    use anchor_runtime::graph::RunStatus;
rust/anchor-runner-host/src/application/liveness.rs:33:            (RunStatus::Ready, false),
rust/anchor-runner-host/src/application/liveness.rs:34:            (RunStatus::Running, false),
rust/anchor-runner-host/src/application/liveness.rs:35:            (RunStatus::Paused, false),
rust/anchor-runner-host/src/application/liveness.rs:36:            (RunStatus::BudgetStopped, false),
rust/anchor-runner-host/src/application/liveness.rs:37:            (RunStatus::WaitingCall, false),
rust/anchor-runner-host/src/application/liveness.rs:38:            (RunStatus::WaitingRecovery, false),
rust/anchor-runner-host/src/application/liveness.rs:39:            (RunStatus::Completed, true),
rust/anchor-runner-host/src/application/liveness.rs:40:            (RunStatus::Aborted, true),
rust/anchor-runner-host/src/application/liveness.rs:41:            (RunStatus::Stopped, true),
rust/anchor-runner-host/src/application/liveness.rs:42:            (RunStatus::Failed, true),
```

全部26行位于cfg(test)：run_projection15行、liveness11行、conversations0、kernel_runs0；生产各0。按标识符而非命中行计数的全Host盘点见counts.json。临时旧适配和JSON边界的RunStatus仍存在，已作为保留成本计入测量。

## 交回边界

自行登记并继续的决定：旧Failed原因码缺失用既有Unclassified；GraphProgress只暴露本刀实际读取的passes；观察写入失败只诊断、读取保持未知；旧JSON的driver重启和quiescent语义单列保留。未新增状态枚举或Kernel字段，也未增加用户审批流程。

R结论为**需要重想**：包含保留/共享适配的82→90点，四消费者生产引用清零不等于规则减少，见[measurement.md](measurement.md)。没有实际修改Kernel/旧JSON、弱化断言或新增状态枚举；按负结论交回，严格单View的旧行为表达边界未关闭，不继续补状态字段或推进H3–H6。

真实Goose/Provider端到端未执行，固定Goose路径和真实Provider未配置；Gate/确定性fixture与真实Op.run/Bubblewrap不能替代真实模型验收。Web和部署未执行。旧P原生Run缺少宿主观察文件时，更新时间查询明确不可用，HTTP迁移恢复尚未验收；控制和生命周期读取已有观察故障回归。现有P不支持的fanout/join、Call、助手、pause、Goose attempt恢复及删除范围仍保留，详见[P未决](../p/measurement.md#未决)。
