# Example: Neural Network Model Development and Training

Status: Proposal
Last updated: February 24, 2026

## Overview

This example describes how to use kelix to autonomously develop, experiment with, and train neural network models. The session is long-lived and processes tasks from a backlog: new model architectures, training runs, hyperparameter searches, and dataset updates arrive as backlog items. Each new experiment campaign or operational request becomes its own work item inside the session.

The key difference from a software project is that **experiments are non-linear**: multiple training runs execute in parallel, results determine which directions to pursue, and the orchestrator must track experiment state — not just task completion status. All artifacts (code, configs, metrics, checkpoints) are versioned in git; large binary files (weights, datasets) are tracked via references only.

## Architecture

```
User / Research notes / Scheduled retraining
        │
    adapter (event router)
        │
   core (one session per model project, long-lived)
        │
   orchestrator (long-lived, holds experiment state + active work item)
     ├── planning-agent    (experiment design, hyperparameter search strategy)
     ├── research-agent    (literature search, architecture survey)
     ├── data-agent        (dataset preparation, validation, versioning)
     ├── train-agent       (training run execution, checkpoint management)
     ├── eval-agent        (metric evaluation, result comparison)
     ├── coding-agent      (model architecture code, training scripts, tooling)
     └── review-agent      (code review gate for architecture and pipeline changes)
```

## Repository Layout

**Meta-repo** (one per model project): owned by the orchestrator.

```
meta-repo/
  .kelix/
    session-state.json
    work-items/
      work-010/
        plan-001.json
  project/
    backlog.md
    roadmap.md
    experiments/
      index.md                         # experiment registry: id, status, metrics summary
      exp-001-baseline/
        config.yaml                    # training config committed at dispatch time
        metrics.json                   # final eval metrics committed by eval-agent
        notes.md                       # orchestrator observations and next steps
      exp-002-larger-lr/
        config.yaml
        metrics.json
        notes.md
    datasets/
      registry.json                    # dataset name → storage URL + content hash
    models/
      registry.json                    # model name → checkpoint storage URL + metadata
    adr/
      001-architecture-choice.md
      002-data-augmentation-strategy.md
  reports/
    research-<topic>.md
```

**Model repo** (one per model): owned by coding-agent. Contains model architecture code, training scripts, and CI pipeline. No binary artifacts.

```
model-repo/
  src/
    model.py
    train.py
    eval.py
    data.py
  configs/
    base.yaml
    experiments/                       # per-experiment config overrides
  .kelix/
    ci.sh                              # lint, unit tests, dry-run training step
  tests/
```

Large artifacts (dataset files, model checkpoints) are stored in an object store (S3, GCS, or local NAS). Only URLs and content hashes are committed to git.

**Repo and infra configuration is a deployment-layer concern.** The URLs for meta-repo, model-repo, and object store, along with git credentials and object store access keys, are baked into the orchestrator and worker container images (or injected via volumes). kelix core does not know about or manage any of these. Workers receive repo URLs and other infra context via the `spawn` input payload, as set by the orchestrator's system prompt and session state.

## Subagent Responsibilities

**planning-agent**: optionally decomposes the active work item's research goal into concrete experiments. Designs hyperparameter search strategies (grid, random, Bayesian). Reads `experiments/index.md` to understand what has already been tried and proposes the next set of experiments. Commits experiment configs to meta-repo before dispatch.

**research-agent**: surveys literature and open-source models for relevant architectures, training tricks, and dataset choices. Commits research reports to `meta-repo/reports/`. Consulted at the start of a new architecture exploration or when eval results plateau.

**data-agent**: prepares and validates datasets. Downloads raw data, runs preprocessing pipelines, validates data quality (schema, class balance, deduplication), and registers the final dataset version in `meta-repo/project/datasets/registry.json`. Operates in an isolated clone of the model repo. Does not run training.

**train-agent**: executes training runs. Receives a training config and dataset reference; clones the model repo; runs the training script; uploads checkpoints to the object store; commits the checkpoint URL and training log summary to the experiment directory in meta-repo. Does not evaluate results — that is eval-agent's responsibility.

**eval-agent**: evaluates a trained checkpoint against the validation and test sets. Loads the checkpoint from the object store; runs the eval script; commits `metrics.json` to the experiment directory. Compares results against previous experiments in `experiments/index.md` and flags regressions. Returns a structured result: `{ experiment_id, metrics, vs_baseline, recommendation }`.

**coding-agent**: implements model architectures, training scripts, data pipelines, and evaluation harnesses. Also maintains `.kelix/ci.sh`. Commits to task branches; never pushes directly to main.

**review-agent**: reviews code changes (architecture, training script, data pipeline). Does not evaluate model quality — that is eval-agent's job. Checks that experiment configs are reproducible (fixed seeds, pinned dependency versions, dataset hash recorded).

## Full Pipeline

### 1. New Architecture Exploration

```
user: "explore transformer-based encoder for time-series classification"
  → orchestrator reads experiments/index.md, backlog, datasets/registry.json
  → orchestrator creates new work item work-010 ("Transformer exploration for time-series classification")
  → orchestrator spawns research-agent (survey transformer architectures for time-series)
  → research-agent commits report to meta-repo/reports/
  → orchestrator spawns planning-agent with the work item goal, research report, and current experiment history
  → planning-agent returns plan version 1 for work-010:
      exp-010: baseline LSTM (control)
      exp-011: vanilla transformer encoder
      exp-012: PatchTST variant
  → experiment configs committed to meta-repo/project/experiments/
  → plan persisted as .kelix/work-items/work-010/plan-001.json
  → plan_gate: human reviews and approves
```

### 2. Parallel Training Runs

The orchestrator dispatches all independent experiments simultaneously:

```
orchestrator spawns train-agent for exp-010 (LSTM baseline)
orchestrator spawns train-agent for exp-011 (transformer)
orchestrator spawns train-agent for exp-012 (PatchTST)

each train-agent:
  → clones model repo
  → reads config from meta-repo/project/experiments/exp-<id>/config.yaml
  → runs training script with GPU resource allocation
  → uploads checkpoint to object store
  → commits checkpoint URL + training log to meta-repo
  → exits with: { experiment_id, status: success, checkpoint_url, epochs_completed }
```

GPU resource contention is managed by `max_concurrent_spawns` in core config (set to match available GPU count). train-agent containers request GPU devices via container flags (`--device nvidia.com/gpu=1`).

### 3. Evaluation and Comparison

```
orchestrator spawns eval-agent for each completed training run
  → eval-agent loads checkpoint, runs eval script
  → commits metrics.json to experiment directory
  → updates experiments/index.md with result row
  → returns recommendation: { continue | prune | promote }

orchestrator reads all eval results:
  → prunes experiments below threshold (marks as pruned in index.md)
  → promotes best-performing experiment as new baseline
  → sends summary to user: "exp-012 (PatchTST) best at 87.3% F1, +3.1% vs LSTM baseline"
```

### 4. Iteration: Hyperparameter Search

```
user: "run hyperparameter search on exp-012 config"
  → orchestrator creates new work item work-011 ("Hyperparameter search for exp-012")
  → orchestrator spawns planning-agent (design search: lr, dropout, patch_size)
  → planning-agent produces search grid (8 configs) as plan version 1 for work-011, marks each train task
    parallel_safe with conflict_domains scoped per GPU slot / experiment id,
    and commits the plan to meta-repo
  → plan_gate: human approves (or auto if plan_gate=none)
  → orchestrator dispatches all ready train tasks whose conflict_domains do not overlap,
    capped by max_concurrent_spawns
  → eval-agent scores each run
  → orchestrator identifies best config, promotes to new baseline
  → orchestrator may spawn planning-agent again to revise work-011 or to record findings as ADR
```

### 5. Architecture Code Change

```
user: "add rotary positional embeddings to the transformer encoder"
  → orchestrator spawns coding-agent (model repo, task branch task/rotary-pos-embed)
  → coding-agent implements change, writes unit tests
  → coding-agent runs .kelix/ci.sh (lint, tests, dry-run training step)
  → exits with: { status: success, branch: "task/rotary-pos-embed" }
  → orchestrator spawns review-agent
  → review-agent approves
  → merge_gate: human confirms
  → orchestrator merges to main
  → orchestrator adds training task to backlog: "run exp-013 with rotary embeddings"
```

### 6. Dataset Update

```
user: "add Q4 2025 data to the training set"
  → orchestrator spawns data-agent
  → data-agent downloads raw data, runs preprocessing pipeline
  → validates quality: schema check, class balance, deduplication against existing splits
  → registers new dataset version in meta-repo/project/datasets/registry.json:
      { "name": "timeseries-v3", "url": "s3://...", "sha256": "...", "split": {...} }
  → orchestrator adds to backlog: "retrain baseline on timeseries-v3"
```

### 7. Model Promotion to Production

```
user: "promote exp-012-best to production"
  → orchestrator checks eval metrics meet promotion threshold (defined in backlog/roadmap.md)
  → orchestrator sends approve(kind=merge, message="Promote exp-012-best checkpoint?")
  → human approves
  → orchestrator commits model entry to meta-repo/project/models/registry.json:
      { "name": "ts-classifier-v2", "checkpoint_url": "s3://...", "metrics": {...}, "promoted_at": "..." }
  → orchestrator spawns deploy-agent (if serving infrastructure exists)
```

## Experiment State Tracking

The experiment index (`meta-repo/project/experiments/index.md`) is the orchestrator's primary planning surface. It is updated after every eval run and after every promotion or pruning decision.

```markdown
## Experiment Index

| ID      | Description           | Dataset      | Status   | F1    | vs Baseline |
|---------|-----------------------|--------------|----------|-------|-------------|
| exp-010 | LSTM baseline         | timeseries-v2 | complete | 84.2% | —           |
| exp-011 | Vanilla transformer   | timeseries-v2 | pruned   | 82.1% | -2.1%       |
| exp-012 | PatchTST              | timeseries-v2 | promoted | 87.3% | +3.1%       |
| exp-013 | PatchTST + rotary PE  | timeseries-v2 | running  | —     | —           |
```

Experiment statuses: `pending` → `running` → `complete` | `pruned` | `promoted`.

This mirrors the task status model in `session-state.json` but is separate: experiment state is domain-level (model quality, dataset version) while session state is coordination-level (active work item, spawn IDs, retry counts, plan revisions).

## Core Config

```toml
[agent]
max_spawns            = 0    # not enforced; project is long-lived
max_concurrent_spawns = 4    # matches available GPU count
max_wall_time_secs    = 0    # not enforced

[subagents.orchestrator]
command   = "podman run --rm -i my-orchestrator-image"
lifecycle = "session"

[subagents.planning-agent]
command   = "podman run --rm -i --cpus=2 --memory=4g my-planning-agent-image"
lifecycle = "task"

[subagents.research-agent]
command   = "podman run --rm -i --network=host --cpus=1 --memory=2g my-research-agent-image"
lifecycle = "task"

[subagents.data-agent]
command   = "podman run --rm -i --network=host --cpus=4 --memory=16g my-data-agent-image"
lifecycle = "task"
volume    = "dataset-cache-vol"   # local dataset cache across runs

[subagents.train-agent]
command   = "podman run --rm -i --device nvidia.com/gpu=1 --cpus=8 --memory=32g --network=host my-train-agent-image"
lifecycle = "task"
# GPU device allocation; adjust per hardware. Object store access requires --network=host.

[subagents.eval-agent]
command   = "podman run --rm -i --device nvidia.com/gpu=1 --cpus=4 --memory=16g --network=host my-eval-agent-image"
lifecycle = "task"

[subagents.coding-agent]
command   = "podman run --rm -i --cpus=4 --memory=8g my-coding-agent-image"
lifecycle = "task"

[subagents.review-agent]
command   = "podman run --rm -i --cpus=2 --memory=4g my-review-agent-image"
lifecycle = "task"

[tools.shell]
enabled          = true
timeout_secs     = 120    # shell command timeout, not train-agent runtime; see §GPU Resource Management
max_output_bytes = 131072
allowed_commands = ["podman", "git", "python3", "rg", "ls", "cat", "aws", "gcloud", "bash", "kubectl"]

[approval]
shell_gate = "agent"   # approval-agent handles routine training dispatches
plan_gate  = "human"   # human reviews experiment plans before dispatch
merge_gate = "human"   # human confirms model promotions and architecture merges

[budget]
max_tokens         = 0
on_budget_exceeded = "reject_spawn"

[plan]
plan_reviewers = ["planning-agent", "eval-agent"]
# eval-agent participates in plan reflection to catch evaluation methodology issues
# before experiments are dispatched
```

## GPU Resource Management

Training workloads require GPUs, which may be spread across multiple machines. kelix core is agnostic to deployment topology: the `[subagents.<name>].command` field is an arbitrary shell command, so GPU allocation is entirely a deployment-layer concern. Two patterns are described below.

### Pattern A: Single host or small cluster — podman remote

For a single workstation or a small set of GPU nodes reachable via SSH, use `podman --remote` to dispatch containers to specific hosts. Core runs on a CPU-only control node; GPU containers run on remote nodes.

```toml
[subagents.train-agent]
command   = "podman --remote --connection gpu-node-1 run --rm -i --device nvidia.com/gpu=0 --cpus=8 --memory=32g --network=host my-train-agent-image"
lifecycle = "task"
```

For a pool of identical GPU nodes, define one `[subagents.train-agent-*]` entry per node (or per GPU) and set `max_concurrent_spawns` to the total GPU count. The orchestrator spawns against any available train-agent entry; core's concurrency limit prevents oversubscription.

```toml
# Two nodes, one GPU each — max 2 concurrent training runs
[subagents.train-agent-node1]
command = "podman --remote --connection gpu-node-1 run --rm -i --device nvidia.com/gpu=0 --cpus=8 --memory=32g --network=host my-train-agent-image"
lifecycle = "task"

[subagents.train-agent-node2]
command = "podman --remote --connection gpu-node-2 run --rm -i --device nvidia.com/gpu=0 --cpus=8 --memory=32g --network=host my-train-agent-image"
lifecycle = "task"

[agent]
max_concurrent_spawns = 2
```

The planning-agent's system prompt should assign specific `subagent` names (e.g. `train-agent-node1`) per experiment task when node affinity matters (e.g. data locality). Otherwise it uses a single logical name and lets `max_concurrent_spawns` throttle concurrency.

### Pattern B: Kubernetes cluster

For larger clusters, Kubernetes handles GPU scheduling natively. The `[subagents.train-agent].command` points to a **wrapper script** provided by the user as part of their local infra setup rather than a direct container command. This is necessary because multi-step k8s operations (`kubectl apply`, `kubectl wait`, `kubectl logs`) would require shell chaining, which the shell policy gate rejects. The wrapper script encapsulates the full job lifecycle as a single executable.

```toml
[subagents.train-agent]
command   = "bash ~/.local/share/kelix/scripts/train-agent-k8s.sh"
lifecycle = "task"
```

The wrapper script (installed locally at `~/.local/share/kelix/scripts/train-agent-k8s.sh`) reads the spawn input from stdin, generates a Kubernetes Job manifest, submits it, waits for completion, and streams the result back to stdout:

```bash
#!/usr/bin/env bash
# ~/.local/share/kelix/scripts/train-agent-k8s.sh
set -euo pipefail

INPUT=$(cat)   # spawn input JSON from core via stdin
EXP_ID=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['context']['experiment_id'])")
JOB_NAME="train-${EXP_ID}-$(date +%s)"

# Generate Job manifest from template and spawn input
python3 ~/.local/share/kelix/scripts/render-train-job.py "$INPUT" "$JOB_NAME" | kubectl apply -f -

# Wait for completion (no timeout: training runs as long as it needs)
kubectl wait job/"$JOB_NAME" --for=condition=complete --timeout=720000s

# Collect result from job pod stdout and forward to core
kubectl logs job/"$JOB_NAME"

# Clean up
kubectl delete job/"$JOB_NAME" --ignore-not-found
```

The Job manifest requests GPU resources via standard Kubernetes resource limits:

```yaml
resources:
  limits:
    nvidia.com/gpu: "1"
  requests:
    nvidia.com/gpu: "1"
```

GPU scheduling (node selection, multi-GPU jobs, fractional GPU via MIG) is handled by the Kubernetes scheduler and device plugin — no changes to kelix core are required.

`max_concurrent_spawns` still acts as a coarse upper bound on simultaneous training jobs submitted to the cluster. For large clusters where the scheduler handles all contention, set it to `0` (not enforced) and rely entirely on Kubernetes resource quotas.


## Design Notes

**Experiment state vs session state**: `session-state.json` tracks spawn coordination (active work item, in-flight, retries, blocked). `experiments/index.md` tracks domain state (metrics, dataset version, promotion status). The orchestrator maintains both; they are not merged because their lifecycles and consumers differ.

**Binary artifacts never in git**: checkpoints and dataset files are stored in an object store and referenced by URL + content hash. This keeps git history fast and the meta-repo small regardless of model size. The hash is the integrity guarantee; the URL is the retrieval pointer.

**GPU scheduling via `max_concurrent_spawns`**: core's concurrency limit is a coarse upper bound on simultaneous training jobs, not a sufficient scheduling rule by itself. Set it to match the number of available GPUs or scheduler slots. Actual parallel dispatch still depends on the plan's `parallel_safe` and `conflict_domains` declarations. For heterogeneous hardware (some nodes with 1 GPU, some with 4), the deployment layer manages allocation; core only enforces the cap.

**Reproducibility as a review requirement**: review-agent's system prompt requires that every experiment config records a fixed random seed, pinned dependency versions (requirements.txt or lockfile), and a dataset content hash. Configs missing these fields are rejected at review time, before any training run is dispatched.

**Long training runs and core restarts**: `tools.shell.timeout_secs` applies to individual shell commands, not to spawn lifetime. train-agent processes run for as long as they need — core waits indefinitely for a `spawn_result`. For multi-day runs, the train-agent checkpoints periodically and exits with a `handover` payload pointing to the latest checkpoint; the orchestrator re-dispatches from the checkpoint. Core restart during a training run is handled via the standard recovery path: `in_flight` tasks in `session-state.json` are waited on for 60 seconds, then marked `failed` and re-dispatched by the orchestrator.
