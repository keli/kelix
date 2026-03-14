# Example: Autonomous Trading System Pipeline

Status: Proposal
Last updated: February 24, 2026

## Overview

This example describes how to use kelix to autonomously manage the full lifecycle of an algorithmic trading system: framework development, strategy authoring, backtesting, ML model training, live deployment, data maintenance, and production monitoring — all within a single long-lived session.

The session never sends `complete`. It processes a continuous backlog driven by user requests, scheduled data updates, and production alerts. Each strategy initiative, retraining request, or incident response becomes a separate work item. The orchestrator holds the full project context and coordinates all work.

The trading framework is a Python codebase that runs identically in backtest, paper, and live modes — the only difference is configuration. Strategy deployment is therefore a configuration change, not a code change. This simplifies the approval model: deploying a strategy to live means merging a config update, subject to the same review and merge gate as any other change.

## Architecture

```
User / Chat
Scheduled data updates (adapter cron)
Live trading alerts (broker webhook → adapter)
        │
    adapter (event router)
        │
   core (one session, long-lived)
        │
   orchestrator (long-lived, holds full project state + active work item)
     ├── planning-agent     (task decomposition, strategy design)
     ├── research-agent     (market research, literature, data sources)
     ├── knowledge-agent    (trading domain rules, risk limits, API contracts)
     ├── coding-agent       (framework dev, strategy code, backtest scripts)
     ├── backtest-agent     (run backtests, produce performance reports)
     ├── train-agent        (ML model training, checkpoint management)
     ├── eval-agent         (model evaluation, strategy eval, comparison)
     ├── review-agent       (code review, risk parameter check)
     ├── data-agent         (historical data fetch, validation, registry update)
     └── monitor-agent      (live position monitoring, P&L, alert triage)
```

## Repository Layout

**Meta-repo**: owned by the orchestrator. All session coordination state and project-level registries live here.

```
meta-repo/
  .kelix/
    session-state.json
    work-items/
      work-021/
        plan-001.json
  project/
    backlog.md
    roadmap.md
    components.json              # component repo URLs
    contracts/
      framework-api.md           # public interface contract for the trading framework
    strategies/
      registry.json              # strategy name → repo, config, status (paper|live|retired)
    datasets/
      registry.json              # dataset name → storage URL + content hash + date range
    models/
      registry.json              # model name → checkpoint URL + metrics + promoted_at
    experiments/
      index.md                   # backtest and ML experiment registry
    adr/
      001-framework-design.md
      002-data-vendor-choice.md
  reports/
    backtest-<strategy>-<date>.md
    monitor-<date>.md
    research-<topic>.md
```

**Component repos**: each independently versioned.

```json
// project/components.json
{
  "trading-framework": "git@github.com:org/trading-framework.git",
  "strategies":        "git@github.com:org/trading-strategies.git",
  "ml-models":         "git@github.com:org/trading-ml-models.git",
  "data-pipeline":     "git@github.com:org/trading-data-pipeline.git"
}
```

**Object store**: historical market data (OHLCV, order book snapshots, alternative data) and ML model checkpoints. Only URLs and content hashes are committed to git; binary files never enter any repo.

## Subagent Responsibilities

**planning-agent**: optionally decomposes the active work item into tasks with explicit `depends_on` chains. For strategy development, it produces tasks spanning multiple components (framework change → strategy implementation → backtest → eval → deploy). Records architectural decisions as ADRs.

**research-agent**: investigates data vendors, market microstructure, relevant academic strategies, and ML techniques. Commits research reports to `meta-repo/reports/`. Consulted before designing a new strategy or choosing a dataset.

**knowledge-agent**: persistent domain knowledge — risk limits, position sizing rules, broker API contracts, known data quality issues, regulatory constraints. Consulted at the start of each planning cycle. Updated whenever domain rules change.

**coding-agent**: implements framework features, strategy code, backtest harness, and data pipeline scripts. Follows the shared-library convention for the framework (see `project/contracts/framework-api.md`). Commits to task branches; never pushes to main directly. Runs `.kelix/ci.sh` before reporting success.

**backtest-agent**: executes backtests. Receives a strategy config, dataset reference, and date range; clones the strategies repo; runs the backtest script; commits a performance report to `meta-repo/reports/`. Does not make judgements about results — that is eval-agent's job.

**train-agent**: trains ML models used as strategy signals or position sizing inputs. Operates identically to the ML training pipeline (see [ml-training.md](ml-training.md)). Registers trained checkpoints in `meta-repo/project/models/registry.json`.

**eval-agent**: evaluates backtest reports and ML model metrics. Compares against existing strategies and models in the registries. Returns a structured recommendation: `{ subject, metrics, vs_baseline, recommendation: continue|reject|promote }`. Also validates that risk parameters (max drawdown, Sharpe floor, position limits) are met before promotion.

**review-agent**: reviews code changes. For strategy config changes, also checks that risk parameters are within limits defined in knowledge-agent's domain rules. Rejects configs that exceed drawdown or leverage limits regardless of backtest performance.

**data-agent**: fetches, validates, and registers market data updates. Checks schema, gap-fills, deduplicates, and registers the updated dataset in `meta-repo/project/datasets/registry.json`. Also maintains the data pipeline code in the data-pipeline repo.

**monitor-agent**: reads live position state, P&L, fill rates, and error logs from the broker API. Runs on a schedule and on demand after alerts. Commits monitoring reports to `meta-repo/reports/monitor-<date>.md`. Surfaces incidents as backlog items. Never places or cancels orders — it is read-only.

## Full Pipeline

### 1. Framework Development

```
user: "add support for multi-leg options orders to the framework"
  → orchestrator reads framework-api.md, knowledge-agent (broker API constraints)
  → orchestrator creates work item work-021 ("Multi-leg options support")
  → orchestrator spawns planning-agent
  → planning-agent returns plan version 1 for work-021:
      task-001: coding-agent — implement multi-leg order type in trading-framework
                (conflict_domains: [framework:order-model, contract:framework-api],
                 breaking change, downstream tasks needed)
      task-002: coding-agent — update strategies repo to use new order type
                (depends_on: task-001, conflict_domains: [strategy:order-usage])
      task-003: backtest-agent — re-run affected strategies (depends_on: task-002)
      task-004: eval-agent — validate no regression (depends_on: task-003)
  → plan_gate: human approves
  → task-001 dispatched; review-agent gates merge; framework-api.md updated in same PR
  → task-002, task-003, task-004 execute in sequence
```

Framework follows the shared-library convention: `project/contracts/framework-api.md` records the public interface. review-agent rejects any framework PR that changes the public interface without updating this file. The planner models this as a `contract:framework-api` conflict domain so downstream tasks cannot overlap the breaking change. Downstream strategy tasks receive the updated contract in their spawn context.

### 2. Strategy Development and Backtest

```
user: "implement a mean-reversion strategy on equity pairs using the ML signal from model-v3"
  → orchestrator reads models/registry.json (confirm model-v3 is promoted)
  → orchestrator reads datasets/registry.json (identify suitable dataset)
  → orchestrator creates work item work-022 ("Pairs mean-reversion strategy with model-v3")
  → orchestrator spawns research-agent if strategy design has open questions
  → orchestrator spawns planning-agent
  → planning-agent produces plan version 1 for work-022:
      task-010: coding-agent — implement PairsMeanReversion strategy in strategies repo
                context: { model_checkpoint: "s3://...", framework_api: "contracts/framework-api.md" }
      task-011: backtest-agent — run backtest on equities-daily-v5, 2020-2025
                (depends_on: task-010)
      task-012: eval-agent — evaluate backtest report, check risk parameters
                (depends_on: task-011)
  → tasks dispatched in dependency order
  → eval-agent returns: { sharpe: 1.8, max_drawdown: 0.12, recommendation: promote }
  → orchestrator updates experiments/index.md
```

### 3. Strategy Promotion to Paper Trading

```
eval-agent recommendation: promote
  → orchestrator spawns review-agent (strategy code + config diff)
  → review-agent checks: risk params within limits, model version pinned, dataset hash recorded
  → review-agent approves
  → merge_gate: human confirms
  → orchestrator merges strategy to main (strategies repo)
  → orchestrator updates strategies/registry.json:
      { "name": "PairsMeanReversion-v1", "status": "paper", "config": "...", "model": "model-v3" }
  → orchestrator spawns coding-agent: update paper trading config to include new strategy
  → config merged → strategy starts running in paper mode (framework picks up new config on restart)
```

No special deployment step — the framework reads `strategies/registry.json` on startup. Promoting to paper is merging a registry entry update.

### 4. Promotion to Live Trading

```
user: "PairsMeanReversion-v1 has been paper trading for 3 weeks, results look good, go live"
  → orchestrator reads monitor reports for PairsMeanReversion-v1 (last 3 weeks)
  → orchestrator spawns eval-agent (evaluate paper trading performance vs backtest)
  → eval-agent: { live_sharpe: 1.6, slippage_vs_backtest: 0.03, recommendation: promote }
  → orchestrator sends approve(kind=merge, message="Promote PairsMeanReversion-v1 to live? Paper Sharpe: 1.6")
  → human approves
  → orchestrator spawns coding-agent: update strategies/registry.json status from paper → live
  → review-agent gates the config change
  → merge_gate: human confirms (second confirmation for live deployment)
  → orchestrator merges; framework picks up live config on next restart
  → orchestrator notifies: "PairsMeanReversion-v1 is now live"
```

Two separate human approvals gate the live promotion: `approve` (go/no-go decision) and `merge_gate` (final confirmation of the config merge). This is intentional — live trading carries real financial risk.

### 5. Historical Data Update

```
adapter cron (daily, market close):
  → user_input: "update equities daily OHLCV to 2026-02-24"
  → orchestrator spawns data-agent
  → data-agent fetches new bars from data vendor
  → validates: schema check, no gaps, prices in expected range
  → uploads to object store, updates content hash
  → commits update to meta-repo/project/datasets/registry.json:
      { "name": "equities-daily-v5", "url": "s3://...", "sha256": "...", "date_range": "2010-2026-02-24" }
  → orchestrator checks backlog for strategies scheduled for re-backtest on new data
  → if any: adds backtest tasks to backlog
```

### 6. ML Model Training and Integration

```
user: "retrain the signal model on the updated dataset"
  → orchestrator reads datasets/registry.json (latest equities-daily-v5 hash)
  → orchestrator creates work item work-023 ("Retrain signal model on updated dataset")
  → orchestrator spawns planning-agent (design training experiment for work-023)
  → train-agent executes (see ml-training.md for GPU scheduling patterns)
  → eval-agent evaluates new checkpoint vs model-v3
  → if improved: orchestrator updates models/registry.json, promotes new model
  → orchestrator adds to backlog: "re-backtest strategies using model-v3 with new model-v4"
  → affected strategies go through backtest → eval → paper → live promotion cycle again
```

The dataset hash in `datasets/registry.json` is the coordination link between data updates and downstream work. The orchestrator reads the registry after every data update and identifies which experiments or strategies reference the updated dataset.

### 7. Live Trading Alert and Incident Response

```
broker webhook → adapter → user_input: "alert: PairsMeanReversion-v1 drawdown 8% in 2h"
  → orchestrator spawns monitor-agent (diagnose: read positions, fills, P&L, market conditions)
  → monitor-agent commits diagnosis to meta-repo/reports/monitor-<timestamp>.md
  → orchestrator sends blocked: "PairsMeanReversion-v1 drawdown 8% in 2h. Diagnosis: <summary>. Halt strategy?"
  → human decides: halt | reduce position | continue monitoring
  → on halt: orchestrator spawns coding-agent — update registry.json status to suspended
             merge_gate: human confirms
             framework picks up suspended status on next cycle
  → incident committed to backlog as post-mortem task
```

The orchestrator never autonomously halts a live strategy — it always escalates to human via `blocked`. This is a hard convention in the orchestrator system prompt, not a core protocol constraint. Configuring `plan_gate: none` and `merge_gate: none` would enable autonomous halting; this is intentionally left as a human decision.

### 8. Scheduled Monitoring

```
adapter cron (every 30 min during market hours):
  → user_input: "run routine monitoring check"
  → orchestrator spawns monitor-agent
  → no issues: monitor-agent commits clean report, session remains suspended
  → issues found: monitor-agent commits incident report
                  orchestrator adds to backlog
                  if critical: orchestrator sends blocked → human decides
```

## Approval Gate Configuration

Different operations carry different risk levels. The approval gates are configured to match:

| Operation | Gate | Rationale |
|-----------|------|-----------|
| Framework code merge | `merge_gate: human` | Affects all strategies |
| Strategy code merge | `merge_gate: human` | Code correctness |
| Promote to paper | `merge_gate: human` | Config change, low risk |
| Promote to live | `approve` + `merge_gate: human` | Real money; two confirmations |
| Data registry update | `merge_gate: agent` | Routine, low risk |
| Model registry update | `merge_gate: human` | Affects strategy signal quality |
| Halt live strategy | `blocked` (always human) | Emergency; never autonomous |

## Core Config

```toml
[agent]
max_spawns            = 0    # not enforced; project is long-lived
max_concurrent_spawns = 6
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

[subagents.knowledge-agent]
command   = "podman run --rm -i my-knowledge-agent-image"
lifecycle = "task"
volume    = "knowledge-vol"

[subagents.coding-agent]
command   = "podman run --rm -i --cpus=4 --memory=8g my-coding-agent-image"
lifecycle = "task"

[subagents.backtest-agent]
command   = "podman run --rm -i --cpus=4 --memory=16g my-backtest-agent-image"
lifecycle = "task"
# No GPU required for backtesting; CPU-bound vectorised simulation

[subagents.train-agent]
command   = "podman run --rm -i --device nvidia.com/gpu=1 --cpus=8 --memory=32g --network=host my-train-agent-image"
lifecycle = "task"
# For multi-node GPU: see ml-training.md §GPU Resource Management

[subagents.eval-agent]
command   = "podman run --rm -i --cpus=2 --memory=4g my-eval-agent-image"
lifecycle = "task"

[subagents.review-agent]
command   = "podman run --rm -i --cpus=2 --memory=4g my-review-agent-image"
lifecycle = "task"

[subagents.data-agent]
command   = "podman run --rm -i --network=host --cpus=4 --memory=8g my-data-agent-image"
lifecycle = "task"
volume    = "data-cache-vol"

[subagents.monitor-agent]
command   = "podman run --rm -i --network=host --cpus=1 --memory=2g my-monitor-agent-image"
lifecycle = "task"
# --network=host required to reach broker API

[subagents.secret-agent]
command   = "podman run --rm -i my-secret-agent-image"
lifecycle = "task"

[tools.shell]
enabled          = true
timeout_secs     = 120
max_output_bytes = 131072
allowed_commands = ["podman", "git", "python3", "rg", "ls", "cat", "bash"]

[approval]
shell_gate = "agent"   # approval-agent handles routine shell commands
plan_gate  = "human"   # human reviews all task plans before dispatch
merge_gate = "human"   # human confirms all merges; see approval gate table above

[budget]
max_tokens         = 0
on_budget_exceeded = "reject_spawn"

[plan]
plan_reviewers = ["planning-agent", "review-agent"]
# review-agent checks risk parameters and interface contracts during plan reflection
```

## Design Notes

**Single session for the full pipeline**: the orchestrator holds the causal chain from data → model → strategy → live trading in a single context. This is intentional: the orchestrator needs to know which model version a strategy uses, which dataset it was backtested on, and what the live paper performance was before recommending promotion. Splitting into multiple sessions would scatter this context across separate meta-repos with no shared coordination mechanism.

**Deployment is a config merge**: the trading framework reads `strategies/registry.json` at startup and routes orders based on the `status` field (`paper | live | suspended`). There is no separate deployment step — promoting a strategy is merging a registry update. This means all the existing code review, merge gate, and audit trail machinery applies to live deployments without any special-casing.

**Monitor-agent is read-only by design**: its container image does not include order placement tools. This is enforced at the image level, not the protocol level. The orchestrator's system prompt also explicitly prohibits spawning any agent with order placement capability in response to an automated alert — only a human `blocked_result` can trigger a config change that affects live positions.

**Broker credentials via secret-agent**: monitor-agent and the live trading framework process receive broker API keys via secret-agent tmpfs volume injection. Keys never appear in git history, session logs, spawn inputs, or meta-repo. The trading framework process itself is outside kelix's scope — it runs as a separate service that reads its config from the strategies repo.

**Data and model registries as coordination surface**: `datasets/registry.json` and `models/registry.json` are the primary mechanism for coordinating between the data pipeline, ML training, backtesting, and strategy development work streams. The orchestrator reads both registries when planning any task that touches data or models. The content hash in each registry entry is the integrity guarantee — downstream tasks that reference a dataset or model by hash are guaranteed to use exactly what was validated.

**Backtest reproducibility**: review-agent's system prompt requires every backtest config to record a fixed random seed, pinned framework commit SHA, dataset content hash, and model checkpoint URL. Backtests missing these fields are rejected before results are accepted. This ensures any backtest result can be reproduced exactly.

**Live trading is outside core's scope**: kelix manages the strategy configuration and the code that runs the trading system, but does not manage the trading process itself. The trading framework is a separate long-running service. kelix's role ends at merging a config change; the framework picks it up on restart or reload. This boundary is intentional — real-time order management has latency and reliability requirements incompatible with an agent coordination loop.
