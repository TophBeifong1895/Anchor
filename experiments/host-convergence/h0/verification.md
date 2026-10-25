# H0 依赖方向围栏交回

2026-10-25，基点 `2152fd9d17e7969dba21666f5bdb414c8bcc4dab`，分支 `host/h0-layer-fence`。开发与规定检查完成，尚待架构负责人验收；未合入 master、未推送、未部署。

## 结果

- 新增 [layer_fence.rs](../../../rust/anchor-runner-host/tests/layer_fence.rs) 与[逐文件上限](../../../rust/anchor-runner-host/layer-fence.txt)，固定三类反向依赖，不加入命名例外。
- 原 Run 围栏中的词法器、生产文件遍历与测试模块排除逻辑移至 [tests/common/mod.rs](../../../rust/anchor-runner-host/tests/common/mod.rs)，删除原位置的副本。Run 围栏的类型集合、豁免、检查规则、原有断言与基线均未改变。
- 新围栏覆盖层次归属、直接路径、分组导入、原始标识符、别名声明、可见性路径与宏输入。两种围栏仍是词法检查，不解析别名使用、相对路径、glob 导入或宏展开。

| 测量 | 前 | 后 |
| --- | --- | --- |
| Host 服务 → 用例 | 58（本次初测） | 58 |
| Host 服务 → 入口适配 | 35（本次初测） | 35 |
| 用例 → 入口适配 | 1（本次初测） | 1 |
| Run 围栏：生产文件 / 业务引用 / 运行时引用 | 174 / 0 / 56 | 174 / 0 / 56 |
| Host 生产物理行数 | 50,958 | 50,958 |

任务书的 73 / 34 / 1 是估算，本次按共用扫描器实际结果建立上限，符合 H0「以测试实际结果为准」的要求。行数复用既有 `measure_lines.py` 的测试分区方法（含所属生产区的注释/空行，不是 SLOC），详见 [line-counts.json](line-counts.json)。所有 `src/` 文件逐字节核对基点，无差异；本项新增内容均为测试、基线与文档。

## 验证

以下命令均已实际执行。完整 Host 输出仅在本机 `/tmp/anchor-host-h0-tests.log`，Clippy 输出在 `/tmp/anchor-host-h0-clippy.log`；表内摘要是本次实际结果，不依赖这些临时文件长期存在。

| 命令 | 结果 |
| --- | --- |
| `scripts/test.sh -p anchor-runner-host --test run_access_fence --locked -- --nocapture`（抽取前） | 1 passed，174 / 0 / 56 |
| `XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runner-host --test run_access_fence --test layer_fence --locked -- --nocapture` | 4 passed；Run 计数不变，层次基线 58 / 35 / 1 |
| `XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runner-host --locked` | 40 个测试二进制合计 1,226 passed / 0 failed / 110 ignored |
| `cargo +stable clippy --manifest-path rust/Cargo.toml -p anchor-runner-host --all-features --all-targets --locked -- -D warnings` | exit 0 |
| `cargo +stable fmt --manifest-path rust/Cargo.toml --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |
| `scripts/check-sensitive-wording.sh` | exit 0 |
| `git diff 9a5fa57 -- rust/anchor-kernel` | 空 |
| `git diff -- rust/anchor-runner-host/run-access-fence.txt` | 空 |

`/run/user/1001` 经 `stat -f` 核实为 tmpfs；只为本次测试设置 `XDG_RUNTIME_DIR`，未改变脚本或产品配置。

### 临时副本负例

从本次工作区复制 Host `src/`、围栏测试与基线至 `/tmp/anchor-host-h0-probe-2_qw3e_b`，用独立 `rustc` 编译同一围栏。仅在副本中新建服务与用例文件，分别添加三类违例；同时在 `assistant.rs` 添加一处引用，验证已登记文件的上限也生效。

实际调用：

```sh
CARGO_MANIFEST_DIR=/tmp/anchor-host-h0-probe-2_qw3e_b \
  rustc +stable --edition=2024 --test \
  /tmp/anchor-host-h0-probe-2_qw3e_b/tests/layer_fence.rs \
  -o /tmp/anchor-host-h0-probe-2_qw3e_b/layer_fence
/tmp/anchor-host-h0-probe-2_qw3e_b/layer_fence --exact layer_fence --nocapture
/tmp/anchor-host-h0-probe-2_qw3e_b/layer_fence --nocapture
```

第二条命令在注入后以 101 失败，列出三类清单外引用与 `assistant.rs` 9 > 8。撤回副本中的注入后，第三条命令 3 passed，计数恢复 58 / 35 / 1。[原始负例输出](negative-probe.log)保留这次预期失败与恢复通过的结果；工作区生产源码未注入违例。

## 边界与偏离

无产品行为、HTTP、持久格式、路径、权限或恢复语义变化。未读取 `.local/`；未运行 ignored Goose 场景、真实 Provider、Web 或部署验收，不把普通测试的通过记作这些验收。除任务书允许的实测基线差异外，H0 无偏离；H1 尚未开始。
