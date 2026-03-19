# Infra Bootstrap Protocol

Status: Proposal
Last updated: March 19, 2026

This document defines orchestrator behavior during infra bootstrap. For the concept overview, see [INFRA_BOOTSTRAP.md](../INFRA_BOOTSTRAP.md).

## 1. Approval Gate Usage

Shell gate policy applies only to commands executed through the core shell execution path (`approve kind="shell"`). Commands an agent executes directly in its own runtime are outside core shell-gate enforcement; limiting that path is a deployment/runtime isolation concern.

Use the following mapping for bootstrap actions:

| Bootstrap action | Core request | Notes |
|---|---|---|
| Execute a shell command (build, apply, push) | `approve kind="shell"` | Routed by `shell_gate` |
| Propose the full bootstrap step plan before any steps run | `blocked` | Use `blocked` to collect one free-form human confirmation on scope/order/rollback before execution begins |
| Propose a config change (image tag, rollout target) requiring free-form input | `blocked` | Use when the decision cannot be expressed as a fixed option list |
| Propose a config change with a fixed option set | `approve kind="shell"` | Include the proposed change in `message`; use `options` for approve/reject/rollback |

## 2. Bootstrap Contract Checklist

Before executing any bootstrap step, define these fields explicitly in the active bootstrap contract:

1. Execution location.
   Example: merge/push on host via core shell; provisioning commands inside orchestrator runtime.
2. Credential source and injection path.
   Example: host `ssh-agent` for host-side push; mounted secret volume for worker runtime.
3. Authoritative state.
   Example: `main` branch in `/workspace` is the single source of integrated state.
4. Failure and escalation policy.
   Example: retry publish once, then escalate; switch to manual intervention after repeated bootstrap failure.
5. Recovery metadata.
   Example: remote, branch, workspace root, bootstrap manifest, and last validated checkpoints in `.kelix/session-state.json`.
6. Bootstrap phase marker.
   Write `bootstrap_phase` (`not_started | in_progress | complete | failed`) and `bootstrap_last_completed_step` to `.kelix/session-state.json` before each step executes. On recovery (`recovery: true`), read this file first and determine whether bootstrap is complete, partially complete, or not started before taking any action. If `bootstrap_phase` is `in_progress` or `failed`, decide whether to resume from `bootstrap_last_completed_step` or roll back before proceeding with normal work.

## 3. Custom Images (Dockerfile)

When a session uses bootstrap workflow, image work is executed by the orchestrator by default.

Expected flow:

1. Generate or update Dockerfile/build context.
2. Build image and run smoke checks.
3. Propose config adoption (image tag/digest, rollout target, rollback target) for approval (`approve kind="shell"` with a fixed option set, or `blocked` if free-form input is needed).
4. After approval, apply the config change and validate runtime health.

Human responsibility is approval and policy control, not manual execution of each build step.

Keep these fields explicit in bootstrap state:

1. Built image identity (`name:tag` and digest).
2. Validation result and required command/tool availability (`git`, `ssh`, toolchain/test binaries).
3. Credential injection path used during runtime.
4. Rollout and rollback target refs.

Outside bootstrap workflow (external infra), image build/publish remains user/deployment managed.

## 4. Abort Handling

On `session_abort` received during bootstrap:

1. Update `bootstrap_phase` to `failed` and write `bootstrap_last_completed_step` to `.kelix/session-state.json`.
2. If the abort reason is not `wall_time_exceeded` and a rollback procedure is defined in the bootstrap contract, attempt rollback before exiting.
3. If rollback cannot complete before exit, record its partial state in session-state so a successor can determine the cleanup required.

## 5. Responsibility Boundary

Core:

- enforces protocol and gate policy
- executes commands only when invoked via core shell path
- persists session lifecycle state

Core does not provision deployment infrastructure, inject repo credentials, or define publication behavior.

Orchestrator:

- plans and executes bootstrap actions for the active setup/session
- requests human approval at configured gates before high-risk actions
- records enough runtime artifacts for recovery and rollback
- on recovery, reads bootstrap state before taking any action

Deployment/example setup:

- provides actual runtime capabilities and credentials
- defines publication path and auth context
