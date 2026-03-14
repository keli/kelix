# Infrastructure Bootstrap

Status: Proposal
Last updated: March 6, 2026

## 1. What Infra Bootstrap Means

Default setup is external infra: runtime dependencies (git server, credentials, workspace authority, network path) are provided outside orchestrator management by the user/deployment.

Infra bootstrap is an in-flow workflow where the orchestrator provisions or binds those dependencies as part of the session, with human approval at defined gates.

In plain terms:

- External infra (default): orchestrator consumes an existing setup.
- Infra bootstrap (special workflow): orchestrator arranges the setup it needs, with approval gates.

## 2. Two Deployment Patterns

1. **External git server** (recommended default; not bootstrap).

   - GitHub/GitLab is already available.
   - Integration/push runs on host via core shell path.
   - Credentials are handled by host setup (`ssh-agent`, git credential helper); worker images do not need to embed git credentials.

2. **In-flow managed git server** (infra bootstrap; experimental).

   - The session provisions or configures a dedicated git server, keys, and related runtime wiring.
   - Orchestrator performs bootstrap steps and requests approvals at defined gates.
   - This path is higher risk and may fail due to environment assumptions. Keep strong approval gates and explicit rollback/escalation rules.

Both patterns are valid in protocol terms. The first is simpler and more predictable.

## 3. What It Is Not

Infra bootstrap is not a replacement for host provisioning.

These remain deployment prerequisites:

- container runtime availability
- base images/binaries
- host credential helpers or SSH agent
- host network and filesystem permissions

## 4. Relation to Onboarding

Onboarding helps users get a working setup quickly by applying a known template with low-risk config choices.

Infra bootstrap adapts runtime contract details for a specific environment — it may change images, credentials, publication paths, and isolation boundaries, and typically requires human approval for higher-risk changes.

## 5. Orchestrator Protocol

For the orchestrator-side rules covering approval gate usage, phase tracking, recovery behavior, and abort handling, see [INFRA_BOOTSTRAP_PROTOCOL.md](protocol/INFRA_BOOTSTRAP_PROTOCOL.md).

