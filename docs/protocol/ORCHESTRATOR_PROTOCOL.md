# Orchestrator Protocol

Status: Proposal
Last updated: March 4, 2026

## 1. Overview

This document defines the orchestrator-worker contract. See [DESIGN.md](../DESIGN.md) for the architecture and [CORE_PROTOCOL.md](CORE_PROTOCOL.md) for the orchestrator-core boundary.

The orchestrator is the first subagent launched by `kelix`. It owns bootstrap, planning, dispatch, review gating, integration, and recovery for the active session.

Workers are stateless per invocation. The orchestrator treats them as black boxes: send prompt, receive structured result.

```
kelix core (shell policy gate, turn loop)
    └── orchestrator               ← session coordination over the active work item's plan
            ├── planning-agent    ← optional task decomposition and dependency analysis
            ├── knowledge-agent   ← domain knowledge volume, consulted on demand
            ├── research-agent    ← external information gathering, stateless
            ├── coding-agent      ← task execution + unit tests in a worker-specific workspace
            └── review-agent      ← reviews branch diff, gate before merge
```

The orchestrator manages a long-lived session as a sequence of **work items**. Each work item has its own goal, task plan, execution state, and plan revision history. The orchestrator maintains the active work item's structured JSON task plan as its working plan. That plan may be produced directly by the orchestrator or delegated to a planning-agent. The orchestrator uses the active plan as the basis for scheduling — task ordering, parallelism, and dependency enforcement — while dispatching tasks whose dependencies are satisfied, tracking in-flight spawns by ID, and waiting for results before proceeding to dependent tasks. If the plan needs revision (e.g. due to a blocking failure), the orchestrator either revises it directly or re-invokes the planning-agent with the failure context and replaces the active work item's plan atomically.

## 2. Agent Roles

All subagents are assumed to be full-capability agents. Roles constrain context, not capability.

**Orchestrator**

Owns session coordination: bootstrap, work-item classification, planning, dispatch, review routing, integration, and escalation. It may only spawn agents registered in `[subagents]` and is the authority that marks work items `completed`, `blocked`, or `abandoned`.

**Planning Agent**

Optional specialized planner. It returns a structured plan for the active work item and exits. It does not classify new work items or close them.

**Knowledge Agent**

Agent with access to a persistent domain-knowledge volume. It is invoked on demand, not by default. Typical uses:

- Before planning, to surface domain constraints.
- During design discussion.
- During review, when domain validation is needed.

**Research Agent**

Optional role for heavy information-gathering tasks. Its purpose is context isolation: gather large external inputs, return a compact summary, and keep that noise out of implementation workers. When used, it appears as a normal task in the plan.

**Coding Agent**

Executes implementation tasks. It writes code and tests when needed and must verify `BUILD_CMD` and `TEST_CMD` before reporting success.

**Review Agent**

Reviews produced output before integration. It receives the diff (or equivalent artifact delta) plus the original task prompt and returns approval or required changes. Review is a hard gate.

## 3. Task Decomposition

Task decomposition is owned by the orchestrator and is always scoped to the active work item. After bootstrap (and optionally consulting the knowledge-agent), it either constructs the task plan itself or delegates plan generation to a planning-agent and adopts the returned plan.

**Task-planning rules:**

- Each task must be scoped to a single coherent concern (one module, one feature slice, one bug fix).
- Task granularity: prefer changes that touch fewer than 10 files and can be reviewed in one pass.
- Tasks that have data dependencies must be ordered explicitly.
- Treat parallelism as an optimization, not a default. A task is serialized unless the plan explicitly marks it safe to run in parallel.
- Every task must declare its conflict surface. Shared interfaces, schemas, generated artifacts, and global files belong in `conflict_domains` so the orchestrator can avoid unsafe overlap.
- The plan format is:

```json
{
  "work_item_id": "work-014",
  "plan_version": 3,
  "replaces_version": 2,
  "goal": "Add CSV invoice export",
  "tasks": [
    {
      "id": "task-001",
      "title": "short human-readable title",
      "prompt": "full prompt sent to the worker",
      "depends_on": [],
      "parallel_safe": false,
      "conflict_domains": ["module:billing"]
    }
  ]
}
```

`parallel_safe` defaults to `false` when omitted. `conflict_domains` is a list of planner-declared contention scopes such as `module:auth`, `schema:billing`, `artifact:openapi`, or `global:workspace-config`.

Once a plan version is established for a work item, the orchestrator must not mutate task `id`, `depends_on`, `parallel_safe`, or `conflict_domains` in place. If changes are needed, it must produce a new plan version for that same work item directly or by re-invoking the planning-agent.

**Planner interaction.** Planner-style workers are still invoked through the normal `spawn` request, but plan exchange is explicit rather than file-based:

- The orchestrator passes the current work item goal and, for revisions, the existing plan in `spawn.input.context` (for example `work_item_id`, `current_plan`, `current_plan_version`, and `revision_reason`).
- A planner returns a normal success result whose structured payload includes `kind: "plan"` and a populated `plan` object.
- The orchestrator recognizes `kind: "plan"` as a plan delivery, validates the `work_item_id` and version transition, then persists the adopted plan in its durable session state.

## 4. Worker Invocation

The orchestrator invokes each worker exclusively via the kelix `spawn` request (see [CORE_PROTOCOL.md](CORE_PROTOCOL.md) §4.1).

The orchestrator evaluates ready tasks whose `depends_on` list is fully satisfied. It dispatches a task immediately only when doing so does not violate the task's concurrency contract. kelix returns `spawn_ack` immediately; the orchestrator tracks in-flight tasks by request `id`. When a `spawn_result` event arrives, the orchestrator matches it to the outstanding request by `id`, first determines whether it is a plan output or a task result, then updates the active work item accordingly and re-evaluates any newly unblocked tasks.

Tasks whose `depends_on` list is not yet fully satisfied must not be dispatched until all predecessors have completed with exit code 0.

By default, ready tasks are still serialized. A task may run concurrently only when all of the following are true:

- `parallel_safe` is `true`
- every currently in-flight task is also `parallel_safe`
- its `conflict_domains` set is disjoint from every currently in-flight task's `conflict_domains`

Global files are modeled as a conflict domain such as `global:workspace-config`, rather than as a separate scheduling rule.

Example — single spawn:

```json
{
  "id": "req-010",
  "type": "spawn",
  "subagent": "coding-agent",
  "input": {
    "prompt": "Implement the rate-limiter described in task-003.",
    "context": {
      "task_id": "task-003",
      "branch": "task/task-003"
    }
  }
}
```

Workers do not need out-of-band context fields (e.g. `shared_interfaces`) to access upstream results. Instead, they rely on the session-level infrastructure contract established for the active session (from external deployment setup or bootstrap workflow). In git-backed sessions, that contract usually gives each worker access to a shared repository that reflects the current merged state at dispatch time. In non-git-backed sessions, it may provide another durable shared workspace. Core does not transmit concrete repo addresses or credentials; those remain a deployment- or bootstrap-workflow concern.

Example — two concurrent spawns (explicitly marked parallel-safe and non-conflicting):

```json
{ "id": "req-010", "type": "spawn", "subagent": "coding-agent", "input": { "task_id": "task-001", ... } }
{ "id": "req-011", "type": "spawn", "subagent": "coding-agent", "input": { "task_id": "task-002", ... } }
```

Both messages may be written to the wire before either `spawn_ack` is received. The orchestrator must be prepared to receive `spawn_result` events in any order.

kelix starts each worker process using the command configured in `[subagents.<name>]`, writes the `input` payload to the worker's stdin, and returns `spawn_ack` immediately. The `spawn_result` event is delivered asynchronously when the worker exits. See CORE_PROTOCOL.md §4.1 and §5.3.

Workers are stateless and cannot query other workers or the orchestrator mid-task. Resource limits, network access, and sandbox policy are determined by the worker's environment configuration, not this protocol.

**Worker infrastructure access:** Workers discover shared session resources through the bootstrap-established infrastructure contract, not through `session_start` payload fields defined by core. In git-backed sessions, workers may `git clone`, commit to a task branch, and `git push` using credentials and remotes that are already available in their runtime environment. In other sessions, workers may instead use a shared volume, object store, or another durable medium. Authentication and concrete endpoints are handled by the deployment layer or by bootstrap-created infra, not by kelix core.

## 5. Worker Output Contract

A worker signals completion by exiting with one of:

| Exit code | Meaning |
|-----------|---------|
| 0 | Success — changes committed to the task branch |
| 1 | Failure — no changes committed, error on stdout |
| 2 | Blocked — worker cannot proceed; see `blocked_reason` |
| 3 | Handover — context limit reached; partial progress committed, continuation required |

On success for `kind: "task_result"`, the worker must have committed all changes to its task branch. The commit message must follow Conventional Commits. The final commit must not leave the build or tests in a broken state. Planner-style `kind: "plan"` outputs return structured planning data instead of a task-branch commit.

The worker writes a structured result to stdout on exit:

```json
{
  "kind": "task_result | plan",
  "work_item_id": "work-014",
  "task_id": "task-001",
  "status": "success | failure | blocked | handover",
  "branch": "task/task-001",
  "base_revision": "rev-123",
  "summary": "one-line description of what was done or what remains",
  "error": "",
  "failure_kind": "implementation | push_failed | build_failed | test_failed",
  "blocked_reason": "approval_required | service_unavailable | insufficient_context",
  "plan_version": 3,
  "replaces_version": 2,
  "plan": {
    "work_item_id": "work-014",
    "plan_version": 3,
    "replaces_version": 2,
    "goal": "Add CSV invoice export",
    "tasks": []
  },
  "handover": {
    "progress": "description of completed work so far",
    "remaining": "description of what still needs to be done",
    "next_prompt": "full prompt for the continuation spawn"
  }
}
```

`kind` defaults to `task_result` when omitted. Planner-style outputs use `kind: "plan"`.

`work_item_id` identifies which work item the result belongs to. It is required for plan outputs and recommended for task outputs in long-lived sessions.

`base_revision` records the integrated revision (git commit, workspace revision id, or equivalent deployment-specific token) that the worker used as its starting point. It is required for `success` and `handover` task results, and may be included for other statuses when useful for recovery.

For `kind: "plan"` outputs:

- `plan` is required and contains the complete proposed plan payload.
- `plan_version` must match `plan.plan_version`.
- `replaces_version` is omitted for an initial plan and required for a revision.
- `task_id`, `branch`, `base_revision`, and `failure_kind` are typically omitted.

`failure_kind` is required when `status` is `failure`; omitted otherwise. It indicates the category of failure:

| `failure_kind` | Meaning | Counts toward `MAX_FIX_ATTEMPTS` |
|---|---|---|
| `implementation` | Worker could not produce a correct implementation | Yes |
| `build_failed` | Implementation exists but `BUILD_CMD` failed | Yes |
| `test_failed` | Build passed but `TEST_CMD` failed | Yes |
| `push_failed` | Worker completed successfully and persisted local output, but failed in an additional publication step (for example remote push or promotion) | No — orchestrator re-invokes the same worker role to resume publication |

`push_failed` failures are retried by re-invoking the same worker role with recovery context from the failed output. The worker is responsible for preserving enough context (local commit id, artifact path, resumable publication step, or equivalent) in its structured output for that retry to succeed. This failure class assumes the output is already durable locally and only the extra publication step failed. If publication retry also fails, the orchestrator escalates according to the active deployment setup rather than treating the task as locally lost work.

`blocked_reason` is required when `status` is `blocked`; omitted otherwise. The orchestrator routes blocked tasks as follows:

| `blocked_reason` | Orchestrator action |
|---|---|
| `approval_required` | Surface to user via kelix `approve` or `blocked` request; resume after response |
| `service_unavailable` | Retry automatically (counts toward `MAX_FIX_ATTEMPTS`); escalate to user if retries exhausted |
| `insufficient_context` | Revise the task prompt and, if needed, regenerate the active work item's plan (optionally via planning-agent); retry once |

In git-backed sessions, the orchestrator may determine which files were changed by inspecting the task branch via `git diff --name-only`. In non-git-backed sessions, changed artifacts are determined from the publication metadata or durable workspace state defined by the bootstrap contract; workers do not need to self-report file lists unless that contract requires it.

## 5.1 Literate Annotation Convention

Workers (primarily coding-agent) annotate source files with `@chunk`/`@end-chunk` comments inline. This is a prompt-layer convention, not a core protocol requirement; core does not read or interpret these annotations.

**Format:** A plain comment in the language's native syntax, immediately before the annotated unit:

```
// @chunk <module>/<concern>
// <prose: responsibility and boundary contract>
<code>
// @end-chunk
```

Chunk names follow `<module>/<concern>` in lowercase kebab-case and are unique within the repository. They serve as stable cross-agent references independent of file paths or line numbers.

**Use by downstream agents:**

- **review-agent**: reads `@chunk` annotations directly from the diff to understand intent and boundary contracts. Issues reference chunks by name (e.g. `chunk:auth/token-validator`).
- **knowledge-agent**: scans source files for `@chunk` annotations and indexes each chunk together with its prose as a unit. Queries return prose and code together, giving consumers both the design rationale and the implementation.

## 6. Review and Integration Protocol

Review and integration is per published task result. The orchestrator decides whether and how to invoke review, based on its configured subagents and the active session's deployment setup.

If a `review-agent` subagent is configured, the orchestrator invokes it after each successful coding-agent result. The review-agent receives the diff (or equivalent artifact delta) and the original task prompt, and returns a standard worker result: `status: success` (approved) or `status: failure` (rejected, with blocking issues in `summary` or `error`).

If `[approval.result_gates.coding-agent]` is configured in `kelix.toml`, core intercepts the coding-agent's `spawn_result` before the orchestrator receives it and routes it through the configured gate automatically (see CORE_PROTOCOL.md §7.1). In this case the orchestrator does not need to spawn a review-agent explicitly — it simply receives a success or failure result.

Either approach produces the same orchestrator-visible outcome: a `spawn_result` with `exit_code: 0` (proceed to integration) or `exit_code: 1` (reject and retry).

Per successful task result, in order:

1. Coding-agent completes and exits with code 0 (either directly, or after passing a result gate).
2. Compare the worker's `base_revision` with the current integrated revision.
3. If the integrated revision advanced since the task started, check whether any tasks merged since `base_revision` overlap the task's `conflict_domains`.
4. If overlapping conflict domains are detected: route the task into a reconcile path by sending it back to a worker with the updated base state, or request a plan revision if the current plan is no longer valid.
5. If no overlapping conflict domains are detected: integrate the approved result into the shared session workspace using the bootstrap-defined publication mechanism.
6. Run `BUILD_CMD` and `TEST_CMD` against the integrated state (integration check).
7. Run `LINT_CMD` and `FORMAT_CHECK_CMD` if defined.
8. If all checks pass: mark the task `merged` and finalize the integration according to the session's publication contract.
9. If any check fails: reject the branch, mark task failed, decide whether to retry or escalate.

A failed or rejected result counts toward `MAX_FIX_ATTEMPTS` for the task. The orchestrator sends feedback to the coding-agent for a fix iteration and returns to step 1.

The publication mechanism, including where integration and `git push` run, is defined by the active infra bootstrap contract for the deployment/example setup.

**Git-backed specialization.** In sessions where bootstrap established a shared git workflow:

- Review input is the task branch diff.
- Step 2 compares the task's `base_revision` against the current `main` head (or the current integration head in a non-`main` workflow).
- Step 5 typically runs `git fetch origin task/<task-id>` and prepares the branch for integration.
- Step 8 typically squash-merges to `main` with message `feat(<task-id>): <summary>`.
- The commands above are representative of a common git-backed setup, not a requirement that execution occurs on the host via core shell.

**Conflict handling:**

- Publication conflicts (for example branch fast-forward failure, object lock contention, or merge-driver conflict): the orchestrator may perform the minimal reconciliation defined by the bootstrap contract. If that fails, it either dispatches a worker to prepare a corrected result or marks the task blocked and escalates.
- Stale-base conflicts (the integration head advanced in overlapping `conflict_domains`): detected at steps 3-4. Route the task through a reconcile path instead of attempting direct integration.
- Semantic conflicts that survive the stale-base check: detected by a failing build or test at step 6. Treat as a check failure.
- Conflicts that require new code or config edits are not resolved by the orchestrator directly; they are routed back through a worker or surfaced as `blocked`.

## 7. Orchestrator Context Window and Handover

The orchestrator is long-lived, so it must keep session summaries compact and hand over before its context window is exhausted. The exact threshold is a prompt-layer convention.

**Handover procedure.** When the orchestrator decides to hand over:

1. Prefer to wait until no spawns are in-flight (or cancel only non-critical ones).
2. Ensure `.kelix/session-state.json` and the current bootstrap infra manifest are up to date in the durable session workspace.
3. Write the handover payload to stdout and exit with code 3.

The handover payload follows the same structure as a worker handover (ORCHESTRATOR_PROTOCOL.md §5), with `status: "handover"` and a populated `handover` object:

```json
{
  "status": "handover",
  "work_item_id": "work-014",
  "summary": "Context window approaching limit. Handing over active work item to successor.",
  "handover": {
    "progress": "Work item work-014 is active. Tasks task-001 through task-004 merged. task-005 (rate limiter) is pending.",
    "remaining": "Dispatch task-005, run review-agent, merge to main, then evaluate whether work-014 can be marked completed.",
    "next_prompt": "You are resuming session sess-abc123. Read .kelix/session-state.json for the active work item state. Your immediate next action: dispatch task-005 (rate limiter implementation) to coding-agent using branch task/task-005."
  }
}
```

**Successor startup.** Core re-spawns a new orchestrator immediately with `recovery: true` and the handover payload in `session_start.handover`. The successor:

1. Reads `.kelix/session-state.json` to reconstruct session state and the active work item.
2. Uses `session_start.handover.next_prompt` as its initial working context.
3. Continues execution from where the predecessor left off.

The successor does not re-run planning for the active work item.

**In-flight spawns during handover.** If the orchestrator exits with code 3 while spawns are in-flight, core buffers the pending `spawn_result` events and delivers them to the successor after it connects, exactly as in the crash recovery path. The successor should handle these buffered results before dispatching new work.

## 8. Retry and Escalation

- A failed or rejected task may be retried up to `MAX_FIX_ATTEMPTS` times total (review rejections and check failures share the same counter).
- After `MAX_FIX_ATTEMPTS` exhausted, the orchestrator stops and surfaces the blocker to the user via kelix TUI approval gate.
- On `blocked` exit, the orchestrator routes based on `blocked_reason` as defined in §5. Only `approval_required` is escalated to the user immediately; the others follow retry or re-planning paths first.
- The orchestrator does not proceed with dependent tasks while a predecessor is in retry or blocked state. Tasks that do not depend on the failed task may continue executing only if they still satisfy the active `parallel_safe` and `conflict_domains` concurrency rules.
- On `handover` exit (exit code 3), the orchestrator immediately re-spawns the same worker using `handover.next_prompt` as the new prompt, with the same `task_id` and `branch`. The worker is expected to have committed partial progress before exiting; the continuation spawn picks up from that commit. Handover re-spawns do not count toward `MAX_FIX_ATTEMPTS`. The orchestrator tracks handover count per task; if it exceeds `MAX_FIX_ATTEMPTS`, the task is treated as failed and escalated.

## 9. Isolation Policy

These are prompt-layer conventions, not core-enforced protocol rules. Worker sandboxing remains a deployment concern.

- Each worker operates in a fresh workspace derived from the bootstrap-established session infrastructure contract. That workspace is discarded after the worker exits.
- In git-backed sessions, the orchestrator is the sole writer to the central repo `main` branch.
- The orchestrator must not include references to other workers' environments or task branches in a worker's spawn input.
