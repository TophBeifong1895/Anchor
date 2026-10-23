# 新难任务：校准记录

shards、payroll、contracts、bugtrail 合进 `eval/` 的提交是 89c7a5f。

## 结果（2026-10-23）

- 参考解在真实 Host 上跑，4 个任务都是满分。
- 基线：每个任务用 DeepSeek flash 跑 3 次，共 12 个 Run，数据在 `.local/harness-hub-p8/harder-pilot`。

| 任务 | 分数 | 平均 token | 平均轮数 | 上下文膨胀 |
| --- | --- | --- | --- | --- |
| shards | 3/3 满分 | 55 万 | 26 | 8 倍 |
| payroll | 3/3 满分 | 50 万 | 22 | 9 倍 |
| bugtrail | 3/3 满分 | 185 万 | 34 | 23 倍 |
| contracts | 3/3 满分 | 245 万 | 49 | 22 倍 |

上下文膨胀指最后一轮请求的上下文是第一轮的多少倍。

12 个 Run 共用 1601 万 token，约 3.5 元。其中 97% 是模型输入，输出只占 1–3%。

陷阱没有起作用：
- shards：动手前先逐个检查了 24 个分片的 BOM、CRLF、引号、`#TOTAL` 行和表头次数；
- bugtrail：先读完整个包，修的是根因，再拿原始代码比对一遍改动；
- payroll 和 contracts：完整读了规范和文档。

结论：任务规则写得完整时，这个模型的质量已到天花板。这批任务更适合当作降成本实验的场景，不适合用来区分质量。

## 复跑命令

会调用真实模型并产生费用。在 Claude Code 输入框粘贴时，命令前面加 `! `：

```
cd /home/mansteinl/Anchor/experiments/harness-hub/eval && mkdir -p /home/mansteinl/Anchor/.local/harness-hub-p8 && (for t in shards payroll contracts bugtrail; do ANCHOR_P0_HOST=/home/mansteinl/Anchor/.local/harness-hub-p7/bin-d5180aa/anchor-runner-host ANCHOR_EVAL_LOCAL=/home/mansteinl/Anchor/.local/harness-hub-p8/harder-pilot ANCHOR_EVAL=/home/mansteinl/Anchor/.local/harness-hub-p7/bin-d5180aa/anchor-eval python3 run.py $t 3; done) > /home/mansteinl/Anchor/.local/harness-hub-p8/harder-pilot.log 2>&1 &
```

修改任务定义后，用下面这条复验参考解。不调模型，应全部满分：

```
cd /home/mansteinl/Anchor/experiments/harness-hub/eval && for t in shards payroll contracts bugtrail; do ANCHOR_P0_HOST=/home/mansteinl/Anchor/.local/harness-hub-p7/bin-d5180aa/anchor-runner-host ANCHOR_EVAL_LOCAL=/home/mansteinl/Anchor/.local/harness-hub-p8/harder-ref ANCHOR_EVAL=/home/mansteinl/Anchor/.local/harness-hub-p7/bin-d5180aa/anchor-eval python3 run.py $t 1 --reference 2>&1 | tail -n 2; done
```
