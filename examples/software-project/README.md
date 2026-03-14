# Example: Autonomous Software Project

Status: Proposal
Last updated: March 2, 2026

## Overview

This example describes how to use kelix to autonomously develop and maintain a multi-component software project from requirements through production, including ongoing operations. It is the primary design target for kelix.

This example uses the **git-backed session profile**: bootstrap establishes the meta-repo, component repos, and any supporting artifact stores needed for builds and releases. The core protocol remains profile-agnostic; the git workflow described here is a prompt-layer convention built on top of the bootstrap contract.

A project session is long-lived and never sends `complete` — it runs indefinitely, processing tasks from a backlog as they are assigned. Each backlog item or user request becomes a distinct **work item** inside the session. The orchestrator is the persistent brain of the project; workers are stateless per task.

## Architecture

```
User / Chat / Issue tracker / Monitor alerts
        │
    adapter (event router)
        │
   core (one session per project, long-lived)
        │
   orchestrator (long-lived, holds project state + active work item)
     ├── planning-agent    (task decomposition, ADR authoring)
     ├── research-agent    (technology research, API docs)
     ├── knowledge-agent   (project domain knowledge, persistent)
     ├── coding-agent      (implementation, tests, CI pipeline)
     ├── review-agent      (code review gate)
     ├── deploy-agent      (deployment execution)
     └── monitor-agent     (production health, log analysis)
```

## Repository Layout

A project uses two layers of repositories:

**Meta-repo** (one per project session): owned by the orchestrator. Stores project-level state and coordination artifacts. Its URL and credentials are deployment-layer concerns, baked into the orchestrator container image or injected via volume.

```
meta-repo/
  .kelix/
    session-state.json       # orchestrator runtime state, active work item, and summaries
    work-items/
      work-042/
        plan-001.json
        plan-002.json
  project/
    components.json          # component name → repo URL mapping
    backlog.md               # pending tasks, priority-ordered
    roadmap.md               # milestones and release targets
    adr/                     # architecture decision records
      001-database-choice.md
      002-api-design.md
  releases/
    v1.0.0.md                # release notes, deployment record
    v1.1.0.md
  reports/
    monitor-<date>.md        # monitor-agent health reports
    research-<topic>.md      # research-agent findings
```

**Component repos** (one per component): owned by coding-agent workers. In this git-backed profile, each worker clones its assigned component repo, works on a task branch, and pushes. The orchestrator integrates approved work by merging to `main`.

```json
// project/components.json
{
  "backend":  "git@github.com:org/project-backend.git",
  "frontend": "git@github.com:org/project-frontend.git",
  "infra":    "git@github.com:org/project-infra.git",
  "sdk":      "git@github.com:org/project-sdk.git"
}
```

Workers discover repo access through the bootstrap-established project contract. In this git-backed profile, the orchestrator may include the relevant component identifier, branch target, and any needed repository context in the spawn prompt. The meta-repo location is an orchestrator concern, not a core protocol field.

## Subagent Responsibilities

**planning-agent**: optionally decomposes the active work item's goal into a structured task plan with explicit cross-component dependencies. Also authors ADRs for significant architectural decisions and publishes them to the meta-repo.

**research-agent**: investigates external APIs, libraries, standards, or competitors before implementation. Publishes research reports to `meta-repo/reports/`. Consulted on demand by the orchestrator when a task has unknowns.

**knowledge-agent**: maintains persistent domain knowledge about the project — business rules, API contracts between components, deployment topology, known constraints. Consulted at the start of each planning cycle. State persists across sessions via a named volume.

**coding-agent**: implements tasks in isolated component workspaces. Writes tests alongside implementation. Also authors and maintains the CI pipeline (see §CI/CD below). In this git-backed profile it publishes to a task branch and never writes directly to `main`.

**review-agent**: reviews published task diffs before the orchestrator integrates them. For cross-component changes (e.g. a backend API change that requires a frontend update), review-agent checks that both sides are consistent before approving either integration.

**deploy-agent**: executes deployment after a release is integrated and approved. Supports multiple deployment targets (see §Deployment). Publishes a deployment record to `meta-repo/releases/`.

**monitor-agent**: reads production metrics, logs, and error rates. Runs on a schedule (via adapter cron) and on demand. Publishes health reports to `meta-repo/reports/`. Surfaces production incidents as new backlog items.

## Full Development Lifecycle

### 1. Requirements → Plan

```
user: "add OAuth2 login to the backend and update the frontend login page"
  → orchestrator reads backlog, roadmap, components.json from meta-repo
  → orchestrator creates new work item work-042 ("OAuth2 login end-to-end")
  → orchestrator consults knowledge-agent (existing auth conventions, API contracts)
  → orchestrator spawns research-agent if OAuth2 library choice is open
  → orchestrator spawns planning-agent with the work item goal + current project context
  → planning-agent returns:
      { kind: "plan", work_item_id: "work-042", plan_version: 1, ... }
  → adopted plan for work-042:
      task-001: implement OAuth2 backend
                (backend repo, depends_on: [], parallel_safe: false,
                 conflict_domains: [api:auth, contract:oauth2-callback])
      task-002: update frontend login page
                (frontend repo, depends_on: [task-001], parallel_safe: true,
                 conflict_domains: [frontend:login-ui])
      task-003: update SDK auth helpers
                (sdk repo, depends_on: [task-001], parallel_safe: true,
                 conflict_domains: [sdk:auth-helper])
  → plan persisted as .kelix/work-items/work-042/plan-001.json and referenced from session-state.json
  → plan_gate: human reviews and approves
```

### 2. Initial Implementation

The orchestrator dispatches only tasks whose dependencies are satisfied and whose concurrency contract allows them to run:

```
orchestrator spawns coding-agent for task-001 (backend component)
  → coding-agent opens the backend workspace defined by the git-backed bootstrap contract
  → implements OAuth2, writes tests
  → CI pipeline passes (see §CI/CD)
  → publishes the result to branch task/oauth2-backend
  → exits with output: { task_id, status: success, branch: "task/oauth2-backend" }

(task-002 and task-003 wait for task-001 to be integrated)
```

### 3. Review and Integration

```
orchestrator spawns review-agent for task-001
  → review-agent reads the published backend artifact diff
  → runs static analysis
  → approves or returns blocked_reason
  → on approval: orchestrator sends approve(kind=merge)
  → merge_gate: human confirms (or auto if merge_gate=none)
  → orchestrator integrates task/oauth2-backend into the backend component's primary line
  → task-001 status → merged in session-state.json under work-042

task-002 and task-003 dependencies now satisfied
  → both are marked parallel_safe
  → conflict_domains are disjoint
  → dispatched in parallel
```

In this git-backed profile, the concrete integration step is a squash-merge of `task/oauth2-backend` into `main`.

### 4. Cross-Component Dependency Handling

When task-002 (frontend) depends on task-001 (backend API), the planning-agent records the interface contract in the task plan:

```json
{
  "id": "task-002",
  "component": "frontend",
  "repo": "git@github.com:org/project-frontend.git",
  "depends_on": ["task-001"],
  "parallel_safe": true,
  "conflict_domains": ["frontend:login-ui"],
  "context": {
    "api_contract": "POST /auth/oauth2/callback → { token, user_id }",
    "note": "backend branch task/oauth2-backend merged to main before this task starts"
  }
}
```

The coding-agent for task-002 receives this contract in its spawn input and implements against it without needing the core protocol to understand repository topology. The fact that the dependency was satisfied by a git merge is specific to this profile; the task model itself only cares that task-001 reached the integrated state. Because task-002 and task-003 declare disjoint `conflict_domains`, the orchestrator may run them concurrently after task-001 merges.

### 5. Release

```
user: "release v1.1.0"
  → orchestrator checks all planned tasks are merged
  → orchestrator opens work item work-043 ("Release v1.1.0")
  → orchestrator spawns planning-agent to draft release notes from git log for work-043
  → release notes written to meta-repo/releases/v1.1.0.md
  → orchestrator sends approve(kind=merge, message="Tag and release v1.1.0?")
  → human approves
  → orchestrator spawns deploy-agent
```

### 6. Deployment

```
deploy-agent receives spawn input:
  {
    "version": "v1.1.0",
    "components": { ... },   # repo URLs and commit SHAs to deploy
    "targets": ["production"],
    "strategy": "rolling"
  }

deploy-agent selects deployment method per component:
  backend  → kubectl set image (k8s rolling update)
  frontend → aws s3 sync + cloudfront invalidation
  infra    → terraform apply

deploy-agent writes deployment record:
  meta-repo/releases/v1.1.0.md ← appended with deployment timestamp, SHA, target
```

Deployment targets are not hardcoded — the deploy-agent image includes all required CLI tools and credentials (injected via secret-agent volume, same pattern as infra-management example). New deployment targets are added by updating the deploy-agent image and system prompt.

### 7. Post-Deploy Monitoring

```
adapter cron (30 min after deploy):
  → new user_input: "run post-deploy health check for v1.1.0"
  → orchestrator spawns monitor-agent
  → monitor-agent reads metrics, error rates, logs
  → no issues: writes clean report, orchestrator notifies user
  → issues found: monitor-agent writes incident report
                  orchestrator adds incident to backlog
                  if critical: orchestrator sends blocked → user decides rollback vs hotfix
```

### 8. Production Incident → Backlog Feedback Loop

```
Prometheus alert → adapter webhook → user_input: "alert: high error rate on /auth/oauth2/callback"
  → orchestrator spawns monitor-agent (diagnose)
  → monitor-agent writes diagnosis to meta-repo/reports/
  → orchestrator opens work item work-044 ("Hotfix OAuth2 callback error rate")
  → orchestrator spawns planning-agent for work-044
  → hotfix tasks added under work-044 with priority: critical
  → plan_gate: human approves hotfix plan
  → coding-agent dispatched immediately (skips normal backlog ordering)
  → review-agent → integration → deploy-agent → monitor-agent (verify fix)
```

## CI/CD: Self-Hosted Pipelines

CI/CD pipelines are code written into each component repo by coding-agent. They are not external services — coding-agent authors them as part of the initial project setup task and maintains them as the project evolves.

**Initial setup task** (dispatched once at project start):

```
task-000: bootstrap CI pipeline for each component
  → coding-agent implements pipeline as code in each repo:
       backend/.kelix/ci.sh    (build, test, lint, docker build, push)
       frontend/.kelix/ci.sh
       infra/.kelix/ci.sh
  → pipeline scripts use only tools in the shell allowlist
  → coding-agent runs the pipeline locally to verify it passes before publishing
```

**Coding-agent convention**: after every implementation publication, coding-agent runs `.kelix/ci.sh` in the component repo before reporting success. If CI fails, coding-agent iterates (up to `MAX_FIX_ATTEMPTS`) before escalating with `exit_code: 2` (blocked).

**Deploy-agent convention**: before deployment, deploy-agent runs `.kelix/ci.sh` on the exact commit SHA being deployed as a final gate.

This means CI is always available even without an external CI service. External CI (GitHub Actions, GitLab CI) can be added later as a parallel gate — deploy-agent checks the external CI status via API before proceeding.

## Orchestrator Backlog Management

The orchestrator is the sole publisher of `meta-repo/project/backlog.md`. It updates the backlog after every task completion, every user input that adds requirements, and every monitor-agent incident report.

Backlog format (markdown, human-readable and machine-parseable):

```markdown
## Backlog

### critical
- [ ] hotfix: OAuth2 callback 500 errors (created: 2026-02-24, incident: reports/incident-001.md)

### high
- [ ] implement rate limiting on /auth endpoints
- [ ] add refresh token support

### normal
- [ ] update SDK documentation
- [ ] migrate legacy session tokens
```

The orchestrator reads the backlog at the start of each work cycle and selects the next task based on priority and dependency readiness. Users add items by sending a message; the orchestrator updates the backlog and publishes the new state.

## Core Config

```toml
[agent]
max_spawns            = 0    # not enforced; project is long-lived
max_concurrent_spawns = 6    # up to 6 parallel worker containers
max_wall_time_secs    = 0    # not enforced

[subagents.orchestrator]
command   = "podman run --rm -i my-orchestrator-image"
lifecycle = "session"

[subagents.planning-agent]
command   = "podman run --rm -i --cpus=2 --memory=4g my-planning-agent-image"
lifecycle = "task"

[subagents.research-agent]
command   = "podman run --rm -i --cpus=1 --memory=2g my-research-agent-image"
lifecycle = "task"

[subagents.knowledge-agent]
command   = "podman run --rm -i my-knowledge-agent-image"
lifecycle = "task"
volume    = "knowledge-vol"   # persists across sessions

[subagents.coding-agent]
command   = "podman run --rm -i --cpus=4 --memory=8g my-coding-agent-image"
lifecycle = "task"

[subagents.review-agent]
command   = "podman run --rm -i --cpus=2 --memory=4g my-review-agent-image"
lifecycle = "task"

[subagents.deploy-agent]
command   = "podman run --rm -i --network=host --cpus=2 --memory=4g my-deploy-agent-image"
lifecycle = "task"
# --network=host for cloud API access and SSH

[subagents.monitor-agent]
command   = "podman run --rm -i --network=host --cpus=1 --memory=2g my-monitor-agent-image"
lifecycle = "task"

[subagents.secret-agent]
command   = "podman run --rm -i my-secret-agent-image"
lifecycle = "task"

[tools.shell]
enabled          = true
timeout_secs     = 300
max_output_bytes = 131072
allowed_commands = ["podman", "git", "kubectl", "helm", "terraform", "aws", "gcloud", "az", "ssh", "rsync", "cargo", "npm", "go", "python3", "rg", "ls", "cat"]

[approval]
shell_gate = "agent"   # approval-agent handles routine commands
plan_gate  = "human"   # human reviews task plans before dispatch
merge_gate = "human"   # in this git-backed profile, human confirms merges to main

[budget]
max_tokens         = 0        # not enforced for long-lived projects
on_budget_exceeded = "reject_spawn"

[plan]
plan_reviewers = ["planning-agent", "review-agent"]
# review-agent also participates in plan reflection to catch
# cross-component interface issues before implementation begins
```

## Design Notes

**Why no `complete`?** A software project is ongoing. The session suspends when the host shuts down and resumes on restart. The orchestrator's `session-state.json`, per-work-item plan history under `.kelix/work-items/`, and `backlog.md` are the source of truth for what remains to be done.

**Multi-repo vs monorepo**: multi-repo is chosen here because it matches real-world team structures and allows component-level access control. The orchestrator's `components.json` serves as the single index. A monorepo works equally well — workers clone the same repo, use different subdirectory paths, and branch names include the component prefix (e.g. `task/frontend/oauth2-login`).

**Worker repo isolation**: in this git-backed profile, each coding-agent worker clones its component repo fresh. Workers for different components never share a filesystem. Cross-component coordination happens through the task plan's `depends_on`, `conflict_domains`, and `context` fields, plus integration-time `base_revision` checks — no direct inter-worker communication.

**Secret handling**: deploy-agent and coding-agent receive credentials via secret-agent tmpfs volume injection. Credentials never appear in git history, session logs, or spawn input prompts.

**Self-extending**: if a required tool is absent from the deploy-agent image (e.g. a new cloud provider CLI), the orchestrator creates a task for coding-agent to implement a wrapper script and publish it to the meta-repo. Subsequent deploy-agent spawns mount the meta-repo and use the wrapper. No image rebuild required for simple cases.

**Shared-library components**: when a component (e.g. a framework or SDK) is depended on by other components in the same project, planning-agent records the public interface contract in `meta-repo/project/contracts/<component>-api.md` and lists downstream update tasks with `depends_on` the framework task. review-agent checks that the contract file is updated on any breaking change before approving framework integration. In this git-backed profile, downstream components pin to a commit SHA; the orchestrator triggers a version-bump task across all consumers when a new framework version is released.
