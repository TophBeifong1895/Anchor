# M2 Call 预准入失败提交

任务书 §8 允许新增 Code 提交种类，要求先登记理由。本切片使用该例外新增 `CallSubmission::AdmissionFailed { reason }`，不改变既有 Call `Settle` 禁令、Bound、Completed、Failed、Uncertain 裁决。

旧 API 用例 `op_call_wait_fails_closed_on_missing_input_map_pointer` 要求已接受的父 Run 结算失败，且 `graph_calls` 为空，因为没有子 Run 获准准入。复制到 Kernel 路径后，原始错误为 `invalid graph snapshot: op.call input_map pointer /nope is not visible from the parent input`；父 Run 留在 Executing，用例在 `parent Run did not settle` 失败。调用组同批其余三个真实 shell API 用例通过。

专用 Code 提交只允许该调用身份尚无持久绑定时使用：宿主在调用端口报告输入或准入校验失败后，取得确定性 child ID 的准入锁，并核查子 Run 资源与准入元数据均不存在，才给出明确的未准入事实。调度器将这一事实提交为 AdmissionFailed。校验器计本次 dispatch 后结算失败，不创建 `graph_calls`。已 Bound 的调用不能用它抹去准入事实；不确定的外部效果仍保持现场，不转换为未准入，也不自动重放。

该提交由 Code 发送；Agent 发送沿用既有权限判定而被拒。原有业务断言与旧 Runner 用例保持原样。
