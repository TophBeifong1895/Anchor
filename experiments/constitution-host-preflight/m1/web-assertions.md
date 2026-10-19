# M1 Web assertion register

Scope: changes from `d915314`; Web production code consumes `actions.pause` independently from stop. No existing tests or business assertions were deleted or relaxed. No ignored tests added. All old fixtures explicitly deny pause (`pause:false`) to retain their original operation facts; new control tests exercise pause authorization separately.

## Modified existing state vocabulary assertions (4)

| File | Test | Old | New | Basis |
| --- | --- | --- | --- | --- |
| apps/web/e2e/platform-rust.spec.ts | native Rust Host owns manual and scheduled Runs, restart recovery, files and control | Run pill `已暂停` | `等待你处理：paused` | M1 pause review decision; S25 void |
| apps/web/e2e/platform-rust.spec.ts | native Rust Host owns manual and scheduled Runs, restart recovery, files and control | scheduled Run list `status=completed` | `projection={phase:settled,outcome:done}` | Existing R2 external vocabulary (S19/S20), no business behavior change |
| apps/web/e2e/pilot-rust.spec.ts | Goose Pilot controls one Rust Run and reads its artifact after the Host restarts | HTTP `control_requested=pause` | HTTP `projection={phase:executing,active:true}` before releasing the current node | External intent field removed in R2; new separate assertion checks the Pilot tool receipt `operation=pause`, so pause acceptance remains verified |
| apps/web/e2e/pilot-rust.spec.ts | Goose Pilot controls one Rust Run and reads its artifact after the Host restarts | Run pill `已暂停` | `等待你处理：paused` | M1 pause review decision; S25 void |

The existing disk `paused`/`completed` assertions remain untouched: M1 preserves the old Runner's internal pause state. Artifact contents, completed-first-node count, exact retained first result, restart, one-Run admission, authorization/conflict, schedule identity, provider call counts and delivery assertions remain intact.

## Added tests (5)

| File | Test | Behavior |
| --- | --- | --- |
| apps/web/src/RunControls.test.tsx | offers pause and stop independently with their distinct boundary semantics | Separate pause/stop controls, boundary explanation, distinct callbacks |
| apps/web/src/RunControls.test.tsx | exposes only abandonment while waiting for an unknown effect | No pause/stop/resume; abandon warns effects are not reversed and invokes abandon |
| apps/web/src/RunControls.test.tsx | does not derive pause authorization from an executing worker | Explicit action denial wins over phase/worker; absent projection grants nothing |
| apps/web/src/RunControls.test.tsx | blocks every projected operation while an action is pending | All four actions disabled while busy, no callback fires |
| apps/web/src/RunDetailPage.test.tsx | keeps an unknown effect waiting for review without offering automatic replay | Effect reason displayed, only abandon shown and invoked |

Added to the existing native Goose e2e: tool receipt `operation=pause`, independently of the displayed phase.

## Fixture schema additions

All changes below only add `pause:false` to the existing `actions` object. The old stop/resume/abandon facts and existing expected values remain unchanged. The inline `Timeline.test.ts` `expect(factsFor(...))` input also adds only this denied action; its expected interval fact object is unchanged.

- apps/web/src/ChannelsPage.test.tsx: default mock Run projection and the server-action / denied-action overrides.
- apps/web/src/RunDetailPage.test.tsx: shared projection fixture and the denied-action override.
- apps/web/src/RunInspector.test.tsx: shared projection fixture.
- apps/web/src/Timeline.test.ts: executing, settled, waiting-human and waiting-effect projection fixtures.
- apps/web/src/parallel.test.ts: shared executing projection fixture.
- apps/web/src/persistentAssistant.test.ts: waiting-human projection fixture.
- apps/web/e2e/fixtures/runProjection.ts: shared default denied-action fixture.
- apps/web/e2e/graph-calls.spec.ts: executing active/inactive projections.
- apps/web/e2e/recovery.spec.ts: human-waiting and executing active/inactive projections.
- apps/web/e2e/pilot.spec.ts: three settled projections.
- apps/web/e2e/plugin-experience.spec.ts: settled projection.
- apps/web/e2e/workbench.spec.ts: executing and settled projections.

## Validation

- `npm --prefix apps/web test`: exit 0; 34 files, 290 tests passed; `/tmp/m1-web-test.log`.
- `npm --prefix apps/web run build`: exit 0; TypeScript (including e2e) and Vite passed; `/tmp/m1-web-build.log`. Existing chunk-size advisory remains.
- `git diff --check -- apps/web`: exit 0.
- Browser native e2e and real Goose/provider not executed by the Web subtask; parent may execute them after building the new Host binary.
