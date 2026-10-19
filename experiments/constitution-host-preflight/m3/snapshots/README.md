# M3 旧侧对照快照

依据任务书 §10.13，在删除旧 Runner 前冻结基线 `4d3d19d4ba120a85526c7d5a8abc00ef0c777eec` 的派发轨迹与完整最终记录。五组共 30,000 个 seed，每个 seed 保留 validator、production scheduler 两个原有 run id / input 命名空间，共 60,000 次旧侧执行。Call 的 durable identity 依赖 run id，因此两套命名空间不能合并。

| 文件 | seed 范围（左闭右开） | 节点失败注入 |
| --- | --- | --- |
| `serial.jsonl.gz` | `[0, 10000)` | 开启 |
| `parallel-failures.jsonl.gz` | `[0, 5000)` | 开启 |
| `parallel-no-failures.jsonl.gz` | `[0, 5000)` | 关闭 |
| `parallel-modules.jsonl.gz` | `[0, 5000)` | 关闭；模块上限定向用例另行保留 |
| `calls.jsonl.gz` | `[0, 5000)` | 关闭；保留既有 Call 结果脚本 |

每个 gzip 解压为 JSONL：首行是版本、生成源文件 SHA256、旧 Runner 基线、seed 范围和命名空间；后续每行是 `seed`、`validator`、`scheduler`。每份已准入结果包含有序 `trace` 和完整原始 `record`，包括所有 Work completion、cursor、分支、计数、边决策、Call 与终态错误；准入失败也有具名记录。原始记录以 JSON Value 保存，不把将删除的生命周期类型固化为快照格式。

快照首提交 `0d029eb` 的比对分两层：fresh old 与冻结 old 的 trace、完整记录一致，仅比较时排除 `started` / `updated` 两个墙钟字段，产物本身保留原值；同一次 Kernel 新执行分别与 fresh old、冻结 old 按原差分规则比对。Kernel 不要求与旧记录逐字节相等。该提交的 S18 `check_failed_run` 函数未改动；函数 SHA256 与逐文件摘要、实际终态数量见 [manifest.json](manifest.json)。

删除切片只读这些冻结事实，通过独立 `FrozenOldRecord` wire 保留历史生命周期和分支 cursor；生产 GraphRunRecord 不解码旧格式。S18 checker 的类型适配使用显式结构投影，首终态、执行事实只出现一次、迟到结果进入历史三个不变量与原有完整 Work/分支事实断言不变。当前 30,000 个 seed 仍按原有差分规则覆盖 validator 与 production scheduler，删除切片验证与原测试去向见 [Runtime 测试映射](../runtime-test-mapping.md) 和 [验证摘要](../runtime-removal-checks.json)。

生成必须显式设置环境变量，普通测试只读取提交的快照，不重写文件。生成命令适用于保留旧 Runner 的快照首提交；后续删除旧实现后，重建应检出该提交：

```sh
CARGO_TARGET_DIR=/tmp/anchor-goose-regression-target-core \
ANCHOR_UPDATE_LEGACY_SNAPSHOTS=1 \
cargo test --manifest-path rust/Cargo.toml -p anchor-runtime --locked \
  old_runner_snapshots -- --nocapture
```

首提交执行双重对照；删除切片执行同一命令时读取冻结对照：

```sh
CARGO_TARGET_DIR=/tmp/anchor-goose-regression-target-core \
cargo test --manifest-path rust/Cargo.toml -p anchor-runtime --locked \
  differential -- --nocapture --test-threads=2
```

生成与验证使用确定性本地端口，没有启动 Goose 或真实模型 Provider。原始命令输出保存在 `/tmp/anchor-m3-snapshots-*.log`，不入库；提交的验证摘要只记录实际命令、结果和原始输出摘要。

生成测试 1 通过；双重差分 10 通过、0 失败，串行、两组并行、模块和 Call 全覆盖；Runtime fmt、diff check 与工程措辞检查通过。命令、覆盖统计、原始输出 SHA256 以及因并行清单变更未启动的 clippy 记录见 [checks.json](checks.json)。该次 clippy 不计通过，首提交独立检出后的工作区检查由总验收补充。

完整工作区验证发现并修正了 feature 合并引起的控制节点 JSON 文本键序差异：仅 join/Call 自动生成的 JSON 摘要按完整结构比较，执行者提交文本逐字比较。其他事实与派发比较保持原规则。受影响的 Runtime 及工作区 library target 回归全部通过，详见 checks.json；原始输出在 `/tmp/anchor-m3/`。
