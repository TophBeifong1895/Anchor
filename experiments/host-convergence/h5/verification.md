# H5 编辑前盘点交回

2026-10-26；基点 `master fcda48c`，分支 `host/h5-reverse-deps`，独立 worktree `/tmp/anchor-host-h5`。**盘点完成，生产开发未开始，行数预算待裁决。** 主工作区未修改，未推送、未部署，未读取 `.local/`。

[逐条盘点与行数预算](inventory.md) 覆盖现有 22 + 23 + 1 处引用和 H3 遗留。当前具体方案净增点估计 +186 行，区间 +114～+280，尚不能满足用户要求的 ≤ 0。遵守编辑前 gate，未选择放宽行数、登记例外或改测试断言。

## 当前实际结果

| 项目 | 基点 | 本次交回 |
| --- | ---: | ---: |
| 服务 → 用例 | 22 | 22，尚未实现 |
| 服务 → 入口适配 | 23 | 23，尚未实现 |
| 用例 → 入口适配 | 1 | 1，尚未实现 |
| Run 围栏业务 / 运行时引用 | 0 / 56 | 0 / 56 |
| 模块外底层 Catalog / Library 类型 | 0 / 0 | 0 / 0 |
| Host 生产物理行数 | 51,256 | 51,256（实际增量 0） |

生产行数使用既有 `measure_lines.py::test_partition`，含生产所属注释与空行。[基线数据](baseline-lines.json) 记录源码 tree 和逐文件结果，[重现脚本](measure-baseline.py) 会先确认 Rust 没有偏离基点。当前所有 Rust 源码、测试与围栏字节都未改变，因此基点计数也就是交回计数；不把预算值当作完成值。

## 实际执行的验证

| 命令 | 结果 |
| --- | --- |
| `CARGO_TARGET_DIR=/home/mansteinl/Anchor/rust/target XDG_RUNTIME_DIR=/run/user/1001 scripts/test.sh -p anchor-runner-host --test layer_fence --test run_access_fence --test plugin_library_fence --test configuration_fence --locked -- --nocapture` | exit 0；7 passed / 0 failed；[完整输出](baseline-fences.log) |
| `python3 experiments/host-convergence/h5/measure-baseline.py` | 51,256；使用 test_partition；[数据](baseline-lines.json)。早期一次内联测量脚本因换行字符串 SyntaxError 失败，修正后测量成功；未产生错误的计数产物 |
| `git diff fcda48c -- rust` | 空；未改生产代码、测试或围栏 |
| `git diff 9a5fa57 -- rust/anchor-kernel` | 空 |
| `scripts/check-sensitive-wording.sh` | exit 0；工程文本无拦截词连写 |
| `git diff --check`、`git diff --cached --check` | 最终 exit 0；首次 staged 检查发现测试日志末尾空行，规范化日志末尾后通过 |
| 盘点编号与预算加总核对 | A01–A22、B01–B23、C01 共 46 行；点估计 +186，区间 +114～+280 |

本次是用户明确要求的编辑前交回，未执行 Host 全量测试、严格 clippy、fmt，也未执行围栏收紧后的反向验证；围栏未收紧。以上不视为这些检查通过，后续真正完成 H5 时仍须全部执行。

## 删除、例外与偏离

- 实际删除路径：无，只有盘点/验证证据与任务书状态更新。
- 预计可删路径：下层默认 RunApplication 构造、返回整个 application 的 reader 入口、重复 load_run 映射、assistant_plugins 租约转发、RunApplication 配置副本及恒等根比较；具体移交归属与预算见清单。Plugin 转发均有调用，仅 install_directory 可改为测试专用。
- 实际保留的命名例外：无新增；现有 22 / 23 / 1 基线尚在，不能视为已接受的例外。
- 偏离最终交付要求：H5 清零、H3 遗留实现、完整检查与反向验证均未完成；任务书未写“开发与规定检查完成，待架构验收”，因为该结论不符合现状。原因是用户要求预算不能同时满足时先停下来交回，此条件先于代码实施与完成状态。
