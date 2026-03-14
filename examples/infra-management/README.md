# Example: Infrastructure Management

Status: Proposal
Last updated: March 1, 2026

## Overview

This example describes how to use kelix to manage homelab and cloud infrastructure. Each infrastructure change is a kelix session. IaC files (Terraform, Ansible, Helm charts) committed to git are the source of truth; the real system state is reconciled against them by an observe-agent.

Before normal planning and task dispatch, the orchestrator may run a bootstrap phase to verify or provision session-level infrastructure. In this example, bootstrap may validate the shared IaC repo checkout, create a temporary state bucket, prepare a named volume for reports, or mint short-lived credentials needed by later workers.

Sessions are triggered in three ways:

1. **User request** — via chat or TUI: "upgrade nginx to 1.27 on the homelab cluster".
2. **External event** — alert webhook from Prometheus/Alertmanager triggers an automated remediation session.
3. **Scheduled job** — cron-triggered drift detection session runs daily.

## Architecture

```
User / Chat
Alert webhook (Prometheus, Alertmanager)
Cron scheduler
        │
    adapter process
        │
   core (one session per change task)
        │
   orchestrator
     ├── observe-agent    (read-only: terraform plan, kubectl diff, ping)
     ├── planning-agent   (change plan decomposition)
     ├── infra-agent      (terraform apply, ansible-playbook, kubectl apply)
     ├── review-agent     (change review before merge)
     └── secret-agent     (vault/bitwarden read-only credential injection)
```

## Subagent Responsibilities

**observe-agent**: read-only. Runs `terraform plan`, `kubectl diff`, `ansible --check`, health checks. Never writes. Returns drift reports and system state summaries committed to git.

**infra-agent**: read-write. Executes `terraform apply`, `ansible-playbook`, `kubectl apply/delete`, `helm upgrade`. During bootstrap it may provision or validate shared session infrastructure. During normal tasks it operates in a worker-specific workspace derived from that bootstrap contract, commits changes to a task branch, and hands them back for orchestrator-managed merge after review.

**secret-agent**: read-only credential provider. Reads secrets from Vault or Bitwarden and writes them to a short-lived tmpfs volume mounted into infra-agent containers. Never commits secrets to git. Secrets are injected per-spawn and discarded on container exit.

**review-agent**: reviews the IaC diff on the task branch before the orchestrator merges to main. For destructive changes (`terraform destroy`, node drain), review-agent always returns `blocked_reason: requires_human` to force the merge gate to escalate.

## Risk Classification

Risk classification is a convention enforced by subagent system prompts, not by kelix core.

| Operation | Risk | Gate behavior |
|-----------|------|---------------|
| `terraform plan`, `kubectl get`, read-only observe | None | Auto-approved (`shell_gate: none`) |
| `terraform apply` (additive) | Low | approval-agent decides (`shell_gate: agent`) |
| `ansible-playbook` (service restart) | Medium | approval-agent decides; review-agent flags for human if scope is broad |
| `terraform destroy`, node drain, data deletion | High | infra-agent sends `blocked` request; orchestrator escalates to human regardless of gate setting |

The infra-agent system prompt defines which operations require a `blocked` request before execution. This is the primary mechanism for enforcing risk boundaries without changing core.

## Drift Detection Flow

```
cron → adapter → new session (prompt: "run drift detection")
  → orchestrator bootstrap (validate repo/workspace, state bucket, secret mounts)
  → orchestrator → observe-agent (terraform plan, kubectl diff)
  → no drift detected → observe-agent commits "drift-report: clean" → complete
  → drift detected    → observe-agent commits drift report
                      → orchestrator sends blocked: "drift detected in <resources>, remediate?"
                      → human approves
                      → planning-agent decomposes remediation into tasks
                      → infra-agent executes per task
                      → review-agent gates each merge
                      → complete
```

## Alert Remediation Flow

```
Prometheus alert → AlertManager webhook → adapter
  → adapter creates new session (prompt: "alert: <alert-name> on <host>, details: <labels>")
  → orchestrator bootstrap (validate shared infra + credentials)
  → orchestrator → observe-agent (diagnose: check metrics, logs, service status)
  → observe-agent commits diagnosis report
  → orchestrator sends blocked: "diagnosis: <summary>. Remediate automatically?"
  → human approves (or approval-agent if shell_gate=agent)
  → planning-agent decomposes remediation
  → infra-agent executes
  → observe-agent verifies recovery
  → complete
```

For known runbooks, the orchestrator system prompt can be configured to proceed without a `blocked` step (approval-agent decides), enabling fully automated remediation for well-understood failure modes.

## Core Config

```toml
[agent]
max_spawns            = 50
max_concurrent_spawns = 3
max_wall_time_secs    = 3600  # 1-hour limit per change session

[subagents.orchestrator]
command   = "podman run --rm -i my-orchestrator-image"
lifecycle = "session"

[subagents.observe-agent]
command   = "podman run --rm -i --network=host --cpus=1 --memory=1g my-observe-agent-image"
lifecycle = "task"
# --network=host required to reach internal services (Prometheus, k8s API, Proxmox)

[subagents.planning-agent]
command   = "podman run --rm -i --cpus=1 --memory=2g my-planning-agent-image"
lifecycle = "task"

[subagents.infra-agent]
command   = "podman run --rm -i --network=host --cpus=2 --memory=4g my-infra-agent-image"
lifecycle = "task"
# SSH keys and cloud credentials injected via secret-agent tmpfs volume

[subagents.review-agent]
command   = "podman run --rm -i --cpus=1 --memory=2g my-review-agent-image"
lifecycle = "task"

[subagents.secret-agent]
command   = "podman run --rm -i my-secret-agent-image"
lifecycle = "task"

[tools.shell]
enabled          = true
timeout_secs     = 300   # infrastructure operations can be slow
max_output_bytes = 131072
allowed_commands = ["podman", "git", "terraform", "ansible-playbook", "kubectl", "helm", "ssh", "vault"]

[approval]
shell_gate = "agent"   # approval-agent handles routine commands
plan_gate  = "human"   # always review the change plan before dispatch
merge_gate = "human"   # always confirm before merging IaC changes to main

[budget]
max_tokens         = 1000000
on_budget_exceeded = "abort"
```

## Everything-as-Code for Infrastructure

All session outputs are committed to the IaC git repository:

| Artifact | Path in repo | Committed by |
|----------|-------------|--------------|
| Drift report | `.kelix/reports/drift-<date>.md` | observe-agent |
| Diagnosis report | `.kelix/reports/diagnosis-<session-id>.md` | observe-agent |
| IaC change | `terraform/`, `ansible/`, `k8s/` | infra-agent (task branch) |
| Session state | `.kelix/session-state.json` | orchestrator |

This gives a complete audit trail: every change, its trigger, its review, and its outcome are in git history.

Session bootstrap artifacts that are needed for recovery should also be recorded durably (for example in `.kelix/session-state.json` or an adjacent infra manifest), so a resumed orchestrator can verify the required shared infrastructure before dispatching more tasks.

## Security Boundaries

- **Secret-agent isolation**: secrets never touch the git repo or core's session log. They are injected into infra-agent containers via a tmpfs volume and discarded on exit.
- **observe-agent is read-only by design**: its container image does not include write tools (`terraform apply`, `kubectl apply`, etc.). Network access is required to reach internal services.
- **infra-agent network access**: `--network=host` is required for SSH and cloud API access. This is a deliberate trade-off; tighter network policies are the responsibility of the deployment layer.
- **Shell allowlist**: destructive commands that are not in `allowed_commands` cannot be executed regardless of what a subagent requests.
