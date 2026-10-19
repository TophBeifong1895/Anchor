# R2 I/M/W 逐项判断清单

源码 `74a4dec`，与 counts.json 相同范围；共享命中只列一次。序号标识语法点，不等于业务规则数量。

| # | 消费者 | 文件:行 | 类型 | 归属 | 源码 |
| --- | --- | --- | --- | --- | --- |
| 1 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:73 | if | Session/authorization and existing projection observation | if !view.worker && !coordinated && identity.is_none() { |
| 2 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:24 | arm | external Phase/Waiter/Outcome serialization | Phase::Executing => ("executing", None, None, None), |
| 3 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:27 | arm | external Phase/Waiter/Outcome serialization | } => ("waiting", Some("human"), None, Some(reason.clone())), |
| 4 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:30 | arm | external Phase/Waiter/Outcome serialization | } => ("waiting", Some("child"), None, view.reason.clone()), |
| 5 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:33 | arm | external Phase/Waiter/Outcome serialization | } => ("waiting", Some("effect"), None, view.reason.clone()), |
| 6 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:36 | arm | external Phase/Waiter/Outcome serialization | } => ("settled", None, Some("done"), None), |
| 7 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:39 | arm | external Phase/Waiter/Outcome serialization | } => ("settled", None, Some("failed"), Some(detail.clone())), |
| 8 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:42 | arm | external Phase/Waiter/Outcome serialization | } => ("settled", None, Some("escalated"), Some(detail.clone())), |
| 9 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:45 | arm | external Phase/Waiter/Outcome serialization | } => ("settled", None, Some("unknown"), Some(detail.clone())), |
| 10 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:48 | arm | external Phase/Waiter/Outcome serialization | } => ("settled", None, Some("aborted"), Some(detail.clone())), |
| 11 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:68 | field_write | Session/authorization and existing projection observation | view.updated = self.run_updated(&record.run_id)?; |
| 12 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:74 | field_write | Session/authorization and existing projection observation | projection.actions.stop = false; |
| 13 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:75 | field_write | Session/authorization and existing projection observation | projection.actions.resume = false; |
| 14 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:76 | field_write | Session/authorization and existing projection observation | projection.actions.abandon = false; |
| 15 | run_projection | rust/anchor-runner-host/src/application/run_projection.rs:78 | field_write | Session/authorization and existing projection observation | projection.actions.resume = projection.actions.resume |
| 16 | conversations | rust/anchor-runner-host/src/application/conversations.rs:123 | if | Session (including unchanged legacy write-back callback) | if let Some(source) = metadata.assistant.as_ref() { |
| 17 | conversations | rust/anchor-runner-host/src/application/conversations.rs:126 | if | Session (including unchanged legacy write-back callback) | if sessions |
| 18 | conversations | rust/anchor-runner-host/src/application/conversations.rs:137 | if | Session (including unchanged legacy write-back callback) | if turn.status == TurnStatus::Running { |
| 19 | conversations | rust/anchor-runner-host/src/application/conversations.rs:155 | if | Session (including unchanged legacy write-back callback) | let completed_record = if turn_status == TurnStatus::Completed { |
| 20 | conversations | rust/anchor-runner-host/src/application/conversations.rs:163 | if | Session (including unchanged legacy write-back callback) | Ok(inbound) if inbound.relation.superseded_by_turn_id.is_some() => return Ok(()), |
| 21 | conversations | rust/anchor-runner-host/src/application/conversations.rs:196 | if | Session (including unchanged legacy write-back callback) | Ok(inbound) if inbound.relation.superseded_by_turn_id.is_some() => Ok(()), |
| 22 | conversations | rust/anchor-runner-host/src/application/conversations.rs:379 | if | Session (including unchanged legacy write-back callback) | if !seen.insert(record.run_id.clone()) { |
| 23 | conversations | rust/anchor-runner-host/src/application/conversations.rs:382 | if | Session (including unchanged legacy write-back callback) | if !self.observed_liveness(&record, active)?.is_quiescent() { |
| 24 | conversations | rust/anchor-runner-host/src/application/conversations.rs:405 | if | Session (including unchanged legacy write-back callback) | None if allow_deleted_children => {} |
| 25 | conversations | rust/anchor-runner-host/src/application/conversations.rs:575 | if | Session (including unchanged legacy write-back callback) | if self.read_view(successor, false)?.is_settled() { |
| 26 | conversations | rust/anchor-runner-host/src/application/conversations.rs:580 | if | Session (including unchanged legacy write-back callback) | if self.has_conversation_successor(&successor.run_id)? { |
| 27 | conversations | rust/anchor-runner-host/src/application/conversations.rs:590 | if | Session (including unchanged legacy write-back callback) | if executed { |
| 28 | conversations | rust/anchor-runner-host/src/application/conversations.rs:594 | if | Session (including unchanged legacy write-back callback) | if let Some(assistant) = successor_metadata.assistant.as_ref() { |
| 29 | conversations | rust/anchor-runner-host/src/application/conversations.rs:598 | if | Session (including unchanged legacy write-back callback) | if !successor |
| 30 | conversations | rust/anchor-runner-host/src/application/conversations.rs:610 | if | Session (including unchanged legacy write-back callback) | if !injected { |
| 31 | conversations | rust/anchor-runner-host/src/application/conversations.rs:660 | if | Session (including unchanged legacy write-back callback) | if metadata.graph_call.is_some() { |
| 32 | conversations | rust/anchor-runner-host/src/application/conversations.rs:665 | if | Session (including unchanged legacy write-back callback) | if let Some(source) = metadata.conversation.as_ref() { |
| 33 | conversations | rust/anchor-runner-host/src/application/conversations.rs:668 | if | Session (including unchanged legacy write-back callback) | if chain |
| 34 | conversations | rust/anchor-runner-host/src/application/conversations.rs:677 | if | Session (including unchanged legacy write-back callback) | if id == metadata.run_id { |
| 35 | conversations | rust/anchor-runner-host/src/application/conversations.rs:681 | if | Session (including unchanged legacy write-back callback) | if view.is_settled() { |
| 36 | conversations | rust/anchor-runner-host/src/application/conversations.rs:689 | if | Session (including unchanged legacy write-back callback) | if Self::graph_identity(&other.bundle_source)? == graph_path |
| 37 | conversations | rust/anchor-runner-host/src/application/conversations.rs:704 | if | Session (including unchanged legacy write-back callback) | if active.values().any(&#124;run&#124; run.graph_path == graph_path) { |
| 38 | conversations | rust/anchor-runner-host/src/application/conversations.rs:710 | if | Session (including unchanged legacy write-back callback) | if id == metadata.run_id |
| 39 | conversations | rust/anchor-runner-host/src/application/conversations.rs:717 | if | Session (including unchanged legacy write-back callback) | if let Some(other) = self.metadata(&id)? |
| 40 | conversations | rust/anchor-runner-host/src/application/conversations.rs:743 | if | Session (including unchanged legacy write-back callback) | let plugin_lease = if bundle.plugins.is_empty() { |
| 41 | conversations | rust/anchor-runner-host/src/application/conversations.rs:749 | if | Session (including unchanged legacy write-back callback) | if let Some(run) = |
| 42 | conversations | rust/anchor-runner-host/src/application/conversations.rs:754 | if | Session (including unchanged legacy write-back callback) | if self.metadata(&request.run)?.is_some() { |
| 43 | conversations | rust/anchor-runner-host/src/application/conversations.rs:762 | if | Session (including unchanged legacy write-back callback) | if chain.first().map(&#124;(metadata, _)&#124; metadata.run_id.as_str()) |
| 44 | conversations | rust/anchor-runner-host/src/application/conversations.rs:773 | if | Session (including unchanged legacy write-back callback) | if self |
| 45 | conversations | rust/anchor-runner-host/src/application/conversations.rs:785 | if | Session (including unchanged legacy write-back callback) | if Self::graph_identity(&metadata.bundle_source)? == graph_path |
| 46 | conversations | rust/anchor-runner-host/src/application/conversations.rs:794 | if | Session (including unchanged legacy write-back callback) | if !bundle |
| 47 | conversations | rust/anchor-runner-host/src/application/conversations.rs:822 | if | Session (including unchanged legacy write-back callback) | if chain.is_empty() { |
| 48 | conversations | rust/anchor-runner-host/src/application/conversations.rs:872 | if | Session (including unchanged legacy write-back callback) | RunStatus::Completed if metadata.assistant.is_none() => TurnStatus::Completed, |
| 49 | conversations | rust/anchor-runner-host/src/application/conversations.rs:163 | arm | Session (including unchanged legacy write-back callback) | Ok(inbound) if inbound.relation.superseded_by_turn_id.is_some() => return Ok(()), |
| 50 | conversations | rust/anchor-runner-host/src/application/conversations.rs:164 | arm | Session (including unchanged legacy write-back callback) | Ok(_) &#124; Err(SessionError::Missing) => {} |
| 51 | conversations | rust/anchor-runner-host/src/application/conversations.rs:165 | arm | Session (including unchanged legacy write-back callback) | Err(error) => return Err(ApplicationError::Storage(error.to_string())), |
| 52 | conversations | rust/anchor-runner-host/src/application/conversations.rs:174 | arm | Session (including unchanged legacy write-back callback) | Ok(_) &#124; Err(SessionError::Missing) => {} |
| 53 | conversations | rust/anchor-runner-host/src/application/conversations.rs:175 | arm | Session (including unchanged legacy write-back callback) | Err(error) => return Err(ApplicationError::Storage(error.to_string())), |
| 54 | conversations | rust/anchor-runner-host/src/application/conversations.rs:189 | arm | Session (including unchanged legacy write-back callback) | Ok(_) &#124; Err(SessionError::Missing) => Ok(()), |
| 55 | conversations | rust/anchor-runner-host/src/application/conversations.rs:190 | arm | Session (including unchanged legacy write-back callback) | Err(conflict @ SessionError::Conflict(_)) => { |
| 56 | conversations | rust/anchor-runner-host/src/application/conversations.rs:196 | arm | Session (including unchanged legacy write-back callback) | Ok(inbound) if inbound.relation.superseded_by_turn_id.is_some() => Ok(()), |
| 57 | conversations | rust/anchor-runner-host/src/application/conversations.rs:197 | arm | Session (including unchanged legacy write-back callback) | Ok(_) => Err(ApplicationError::Storage(conflict.to_string())), |
| 58 | conversations | rust/anchor-runner-host/src/application/conversations.rs:198 | arm | Session (including unchanged legacy write-back callback) | Err(error) => Err(ApplicationError::Storage(error.to_string())), |
| 59 | conversations | rust/anchor-runner-host/src/application/conversations.rs:201 | arm | Session (including unchanged legacy write-back callback) | Err(error) => Err(ApplicationError::Storage(error.to_string())), |
| 60 | conversations | rust/anchor-runner-host/src/application/conversations.rs:404 | arm | Session (including unchanged legacy write-back callback) | Some(child) => pending.push(child), |
| 61 | conversations | rust/anchor-runner-host/src/application/conversations.rs:405 | arm | Session (including unchanged legacy write-back callback) | None if allow_deleted_children => {} |
| 62 | conversations | rust/anchor-runner-host/src/application/conversations.rs:406 | arm | Session (including unchanged legacy write-back callback) | None => { |
| 63 | conversations | rust/anchor-runner-host/src/application/conversations.rs:872 | arm | Session (including unchanged legacy write-back callback) | RunStatus::Completed if metadata.assistant.is_none() => TurnStatus::Completed, |
| 64 | conversations | rust/anchor-runner-host/src/application/conversations.rs:873 | arm | Session (including unchanged legacy write-back callback) | RunStatus::Failed &#124; RunStatus::Aborted => TurnStatus::Failed, |
| 65 | conversations | rust/anchor-runner-host/src/application/conversations.rs:874 | arm | Session (including unchanged legacy write-back callback) | RunStatus::Stopped => TurnStatus::Stopped, |
| 66 | conversations | rust/anchor-runner-host/src/application/conversations.rs:875 | arm | Session (including unchanged legacy write-back callback) | _ => return Ok(()), |
| 67 | conversations | rust/anchor-runner-host/src/application/conversations.rs:807 | field_write | Session (including unchanged legacy write-back callback) | record.plugin_bindings = plugin_bindings; |
| 68 | conversations | rust/anchor-runner-host/src/application/conversations.rs:808 | field_write | Session (including unchanged legacy write-back callback) | record.plugin_bindings_initialized = true; |
| 69 | conversations | rust/anchor-runner-host/src/application/conversations.rs:816 | field_write | Session (including unchanged legacy write-back callback) | metadata.trigger_source = "channel".into(); |
| 70 | conversations | rust/anchor-runner-host/src/application/conversations.rs:817 | field_write | Session (including unchanged legacy write-back callback) | metadata.oauth_owner = Some(oauth_owner); |
| 71 | conversations | rust/anchor-runner-host/src/application/conversations.rs:818 | field_write | Session (including unchanged legacy write-back callback) | metadata.conversation = Some(conversation); |
| 72 | conversations | rust/anchor-runner-host/src/application/conversations.rs:819 | field_write | Session (including unchanged legacy write-back callback) | metadata.channel = channel; |
| 73 | conversations | rust/anchor-runner-host/src/application/conversations.rs:820 | field_write | Session (including unchanged legacy write-back callback) | metadata.attachments = attachments.manifest(); |
| 74 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:21 | arm | internal Kernel-to-legacy record bridge (not external JSON) | Phase::Executing => (RunStatus::Running, None), |
| 75 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:24 | arm | internal Kernel-to-legacy record bridge (not external JSON) | } => (RunStatus::Stopped, Some(reason.clone())), |
| 76 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:25 | arm | internal Kernel-to-legacy record bridge (not external JSON) | Phase::Waiting { .. } => (RunStatus::WaitingCall, None), |
| 77 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:28 | arm | internal Kernel-to-legacy record bridge (not external JSON) | } => (RunStatus::Completed, None), |
| 78 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:31 | arm | internal Kernel-to-legacy record bridge (not external JSON) | } => (RunStatus::Aborted, Some(detail.clone())), |
| 79 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:34 | arm | internal Kernel-to-legacy record bridge (not external JSON) | } => (RunStatus::Failed, Some(detail.clone())), |
| 80 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:37 | arm | internal Kernel-to-legacy record bridge (not external JSON) | } => (RunStatus::BudgetStopped, Some(detail.clone())), |
| 81 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:14 | field_write | internal Kernel-to-legacy record bridge (not external JSON) | record.status = status; |
| 82 | kernel_runs_projection | rust/anchor-runner-host/src/application/run_json.rs:15 | field_write | internal Kernel-to-legacy record bridge (not external JSON) | record.error = error; |
| 83 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:10 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::Ready &#124; RunStatus::Running => Phase::Executing, |
| 84 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:11 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::Paused => Phase::Waiting { |
| 85 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:16 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::Stopped => Phase::Waiting { |
| 86 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:21 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::BudgetStopped => Phase::Waiting { |
| 87 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:26 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::WaitingCall => Phase::Waiting { |
| 88 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:31 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::WaitingRecovery => Phase::Waiting { |
| 89 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:36 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::Completed => Phase::Settled { |
| 90 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:39 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::Failed => Phase::Settled { |
| 91 | shared_support | rust/anchor-runner-host/src/application/legacy_run_view.rs:45 | arm | legacy lifecycle mapping/compatibility (not Session authorization) | RunStatus::Aborted => Phase::Settled { |
| 92 | shared_support | rust/anchor-runner-host/src/application/queries.rs:151 | if | read ownership/Host observation (timestamp is not a control input) | if let Some(runs) = self.kernel_owner(&record.run_id)? { |
