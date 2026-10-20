# 评测任务集

P4 的评测任务集，结论见 [P4 报告](../p4/report.md)。

| 文件 | 内容 |
| --- | --- |
| `spec.py` | 任务的形状、随机种子、Agent 可用命令、结束方式 |
| `tasks.py` | 精确档的前 5 个任务，以及把任务组装成 Graph 的方法 |
| `hard.py` | 精确档的后 4 个任务 |
| `graded.py` | 分级档：fleet、triage；fleet 已知最好解的搜索 |
| `fleet_best.json` | fleet 的已知最好解及其来源 |
| `graders.py` | 评分器，只用标准库，原样嵌入 `grade` Op |
| `solvers.py` | 只读输入文件、独立计算期望值的实现，用来复核 |
| `test_eval.py` | 评分可信性测试：`python3 -m unittest test_eval` |
| `run.py` | 运行、评分、记录 |
| `analyze_p4.py` | 汇总质量和成本，写 `../p4/summary.json` |

常用命令（在本目录下）：

```text
python3 run.py all 1 --reference             # 参考解走真实 Host，应全部满分
python3 run.py exact|graded|<task> N         # 基线运行 N 轮
python3 run.py --regrade <runs 目录>          # 用当前任务定义重新评分
python3 analyze_p4.py
```

需要先编译 `anchor-eval` 和 `anchor-hub`。环境变量 `ANCHOR_P0_HOST` 可以指定 Host 可执行文件。运行使用 P0 的隔离方式，不碰 8077 服务的数据。

任务定义改变时（输入、期望值、检查项），要升级任务的 `version`。评分器配置的摘要会自动变化，并盖在每条评分上。
