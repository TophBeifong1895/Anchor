# H2 Run 元数据下沉交回

2026-10-25，基点 `6d31798`（H1 已验收），分支 `host/h2-run-metadata`，独立 worktree `/tmp/anchor-host-h2`。开发与规定检查完成，待架构验收；未合入 master、未推送、未部署。主工作区保持 `master`，原未跟踪路径未改动。

## 结果与替代关系

- 顶层 [run_metadata.rs](../../../rust/anchor-runner-host/src/run_metadata.rs) 取代并删除 `application/metadata.rs`。Run 身份字段、来源数据结构、存储和文件列表均由同一模块拥有，使用独立 `MetadataError`；`application.rs` 只保留一个逐分类、逐文本不变的 `From` 转换，没有旧模块转发或公共重导出。
- 删除 `RunApplication::graph_identity`，原函数体移成 `run_metadata::graph_identity`，全部调用改用自由函数；路径规范化、符号链接解析与缺失尾路径拼接保持原算法。
- `SessionCall` 和 `SessionContext` 从 `application/session_calls.rs` 原样移入元数据模块，避免新存储继续反向引用用例层。Session 授权、接纳、停止与投递协调留在原用例。`RunTrigger::apply` 仅因跨模块调用将可见性调成 `pub(crate)`。
- execution、assistant、assistant_plugins、Node Host、conversation_resources、渠道和入口适配都读取顶层元数据。WeCom 两处已有 HTTP 错误映射的读取调用现有 `RunApplication::metadata`，在应用边界转换后沿用同一 HTTP 映射；其他服务保持各自原有字符串/GraphError 包装。

[源码核对脚本](check-move.py) 与 [结果](move-check.json) 确认：格式化后，元数据源文件只发生声明的模块所有者、错误类型与可见性替换；Session 数据结构和 Graph identity 函数体保持一致。核对涵盖字段顺序、serde 标记、错误文本、临时文件名、写入/同步/重命名顺序、格式 1/2 读取、旧 channel 检查、子调用幂等比较和 id 列表。改动文件中 1,509 处既有断言仅有模块所有者路径或格式变化，没有新增产品测试或修改断言条件。

## 测量与确认的偏离

| 项目 | 前 | 后 |
| --- | --- | --- |
| Host 服务 → 用例 | 58 | 25（−33） |
| Host 服务 → 入口适配 | 23 | 23 |
| 用例 → 入口适配 | 1 | 1 |
| Run 围栏：生产文件 / 业务引用 / 运行时引用 | 175 / 0 / 56 | 175 / 0 / 56 |
| Host 生产物理行数 | 50,878 | 50,909（+31） |

原任务要求服务 → 用例至少减少 40，纯元数据迁移实测减少 33。用户在本轮明确选择「保持 H2 范围，按减少 33 交回验收」。剩余 25 处指向 Run 门面、RunApplication、用例错误/前置条件及 Plugin 发布租约等已有接口；没有把它们挪入新存储，也没有通过合并导入隐藏计数。H3–H5 继续处理剩余边界。

`layer-fence.txt` 只降低实测下降的 12 个文件上限，归零且没有其它分类引用的行移除；[降低前输出](fences-before-ratchet.log) 列出逐文件变化，[最终输出](fences.log) 无待降低提示。Run 围栏上限没有改动。

行数复用 `experiments/constitution-host-preflight/final/measure_lines.py::test_partition`，只取 Host crate，排除测试文件和 `cfg(test)` 区段；包含生产区所属注释与空行，并非 SLOC。[line-counts.json](line-counts.json) 记录基点与仅含源码/围栏改动的 tree 身份。增加来自独立错误类型、边界转换、模块文档与导入/格式调整，不新增持久字段或机制。

## 实际验证

下表 Cargo 命令均设置 `CARGO_TARGET_DIR=/home/mansteinl/Anchor/rust/target`，复用主工作区构建缓存；测试另设置 `XDG_RUNTIME_DIR=/run/user/1001`，临时数据位于 tmpfs。

| 命令 | 结果 |
| --- | --- |
| `cargo +stable check --manifest-path rust/Cargo.toml -p anchor-runner-host --all-targets --locked` | 最终 exit 0；迁移初次编译发现导入和返回错误类型尚未更新，修正后通过 |
| `scripts/test.sh -p anchor-runner-host --test layer_fence --test run_access_fence --locked -- --nocapture` | 降低上限前后均 exit 0、4 passed；最终 25 / 23 / 1，Run 0 / 56；[输出](fences.log) |
| `scripts/test.sh -p anchor-runner-host --bin anchor-runner-host metadata --locked -- --nocapture` | exit 0；16 passed，包括旧元数据可读和不匹配元数据拦截；[输出](metadata-tests.log) |
| `scripts/test.sh -p anchor-runner-host --locked` | exit 0；1,229 passed / 0 failed / 110 ignored，40 个测试二进制；[结果](test-results.json)、[完整输出](tests.log) |
| `cargo +stable clippy --manifest-path rust/Cargo.toml -p anchor-runner-host --all-features --all-targets --locked -- -D warnings` | exit 0；[输出](clippy.log) |
| `cargo +stable fmt --manifest-path rust/Cargo.toml --all -- --check` | exit 0 |
| `python experiments/host-convergence/h2/check-move.py` | exit 0；[源码与断言核对结果](move-check.json) |
| `git diff --check` | exit 0 |
| `scripts/check-sensitive-wording.sh` | exit 0 |
| `git diff 9a5fa57 -- rust/anchor-kernel` | 空 |
| `git diff HEAD -- rust/anchor-runner-host/run-access-fence.txt` | 空 |

## 未覆盖边界

110 个 ignored 未执行；未新增真实 Provider、Goose ignored、Web 或部署验收。没有读取 `.local/` 下的设置、凭据或运行状态；只查看了 Git 的 worktree 清单。没有改变 HTTP 路由/JSON、磁盘路径与字节、权限或并发/恢复语义。H3 未开始。
