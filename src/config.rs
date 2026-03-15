use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub subagents: HashMap<String, SubagentConfig>,
    #[serde(default)]
    pub adapter: AdapterConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub plan: PlanConfig,
    #[serde(default)]
    pub approval: ApprovalConfig,
}

// @chunk config/adapter-runtime
// Adapter config is project-level runtime metadata. It is intentionally kept
// outside [subagents] because adapter lifecycle is independent from
// session/task worker orchestration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AdapterConfig {
    /// Whether `kelix start` should auto-launch the adapter command.
    #[serde(default)]
    pub autostart: bool,
    /// Selected provider key under [adapter.providers.<name>].
    #[serde(default)]
    pub provider: Option<String>,
    /// Provider registry. Builtin and external providers share one control plane.
    #[serde(default)]
    pub providers: HashMap<String, AdapterProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AdapterProviderConfig {
    Builtin,
    External { start_command: String },
}
// @end-chunk

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub max_spawns: u64,
    #[serde(default)]
    pub max_concurrent_spawns: u64,
    #[serde(default)]
    pub max_wall_time_secs: u64,
    /// Seconds to wait for the orchestrator's first stdout line after session_start.
    /// 0 disables the timeout. Default: 120.
    #[serde(default = "default_orchestrator_startup_timeout")]
    pub orchestrator_startup_timeout_secs: u64,
    /// Seconds of silence from the orchestrator before treating it as stuck and killing it.
    /// Resets each time a line is received. 0 disables the timeout. Default: 0 (disabled).
    #[serde(default)]
    pub orchestrator_inactivity_timeout_secs: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_spawns: 0,
            max_concurrent_spawns: 0,
            max_wall_time_secs: 0,
            orchestrator_startup_timeout_secs: default_orchestrator_startup_timeout(),
            orchestrator_inactivity_timeout_secs: 0,
        }
    }
}

fn default_orchestrator_startup_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubagentConfig {
    pub start_command: String,
    /// Optional command to run when the orchestrator process needs to be stopped.
    /// Executed before killing the child process. Supports the same env-var
    /// expansion as `start_command`. Useful for container runtimes (e.g.
    /// `podman stop kelix-{session_id}`) that don't forward signals to the
    /// container when the outer process is killed.
    #[serde(default)]
    pub stop_command: Option<String>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    pub volume: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    #[default]
    Task,
    Session,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default)]
    pub shell: ShellConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShellConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_output")]
    pub max_output_bytes: usize,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_secs: default_timeout(),
            max_output_bytes: default_max_output(),
            allowed_commands: vec![],
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    120
}

fn default_max_output() -> usize {
    65536
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BudgetConfig {
    #[serde(default)]
    pub max_tokens: u64,
    #[serde(default)]
    pub on_budget_exceeded: BudgetAction,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetAction {
    #[default]
    Abort,
    RejectSpawn,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanConfig {
    #[serde(default = "default_plan_reviewers")]
    pub plan_reviewers: Vec<String>,
    #[serde(default = "default_reflection_rounds")]
    pub max_reflection_rounds: u32,
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self {
            plan_reviewers: default_plan_reviewers(),
            max_reflection_rounds: default_reflection_rounds(),
        }
    }
}

fn default_plan_reviewers() -> Vec<String> {
    vec![]
}

fn default_reflection_rounds() -> u32 {
    3
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ApprovalConfig {
    #[serde(default)]
    pub shell_gate: Gate,
    #[serde(default)]
    pub plan_gate: Gate,
    #[serde(default)]
    pub merge_gate: Gate,
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Gate {
    #[default]
    Human,
    Agent,
    None,
}

pub fn load(path: &Path) -> Result<Config, CoreError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| CoreError::Config(format!("cannot read {}: {e}", path.display())))?;
    let config: Config = toml::from_str(&contents)?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &Config) -> Result<(), CoreError> {
    if config.approval.shell_gate == Gate::Agent
        || config.approval.plan_gate == Gate::Agent
        || config.approval.merge_gate == Gate::Agent
    {
        if config.approval.agent.is_none() {
            return Err(CoreError::Config(
                "approval.agent must be set when any gate is 'agent'".to_string(),
            ));
        }
        let agent_name = config.approval.agent.as_ref().unwrap();
        if !config.subagents.contains_key(agent_name) {
            return Err(CoreError::Config(format!(
                "approval.agent '{agent_name}' is not defined in [subagents]"
            )));
        }
    }
    if config.adapter.autostart {
        let selected = config
            .adapter
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                CoreError::Config("adapter.autostart=true requires adapter.provider".to_string())
            })?;
        if !config.adapter.providers.contains_key(selected) {
            return Err(CoreError::Config(format!(
                "adapter.provider '{}' is not defined under [adapter.providers]",
                selected
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_minimal_config() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[subagents.orchestrator]
start_command = "echo orchestrator"

[subagents.coding-agent]
start_command = "echo worker"
"#
        )
        .unwrap();
        let config = load(f.path()).unwrap();
        assert!(config.subagents.contains_key("orchestrator"));
        assert_eq!(config.tools.shell.timeout_secs, 120);
        assert_eq!(config.tools.shell.max_output_bytes, 65536);
    }

    #[test]
    fn test_gate_agent_requires_agent_field() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[subagents.orchestrator]
start_command = "echo orch"

[approval]
shell_gate = "agent"
"#
        )
        .unwrap();
        assert!(load(f.path()).is_err());
    }

    #[test]
    fn test_defaults() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "").unwrap();
        let config = load(f.path()).unwrap();
        assert_eq!(config.agent.max_spawns, 0);
        assert_eq!(config.agent.orchestrator_inactivity_timeout_secs, 0);
        assert!(config.tools.shell.enabled);
        assert!(config.plan.plan_reviewers.is_empty());
        assert_eq!(config.plan.max_reflection_rounds, 3);
        assert_eq!(config.approval.shell_gate, Gate::Human);
    }

    #[test]
    fn test_load_plan_section() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[plan]
plan_reviewers = ["review-agent"]
max_reflection_rounds = 5
"#
        )
        .unwrap();

        let config = load(f.path()).unwrap();
        assert_eq!(config.plan.plan_reviewers, vec!["review-agent"]);
        assert_eq!(config.plan.max_reflection_rounds, 5);
    }

    #[test]
    fn test_load_adapter_section() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[adapter]
autostart = true
provider = "telegram"

[adapter.providers.telegram]
kind = "builtin"
"#
        )
        .unwrap();

        let config = load(f.path()).unwrap();
        assert_eq!(config.adapter.provider.as_deref(), Some("telegram"));
        assert!(matches!(
            config.adapter.providers.get("telegram"),
            Some(AdapterProviderConfig::Builtin)
        ));
        assert!(config.adapter.autostart);
    }

    #[test]
    fn test_adapter_autostart_requires_provider_registration() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[adapter]
autostart = true
provider = "telegram"
"#
        )
        .unwrap();

        assert!(load(f.path()).is_err());
    }
}
