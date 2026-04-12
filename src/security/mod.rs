pub mod confirm;
pub mod direct;
pub mod whitelist;

use crate::config::schema::SecurityConfig;
use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityMode {
    #[default]
    Direct,
    Confirm,
    Whitelist,
}

impl std::fmt::Display for SecurityMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "direct"),
            Self::Confirm => write!(f, "confirm"),
            Self::Whitelist => write!(f, "whitelist"),
        }
    }
}

impl std::str::FromStr for SecurityMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "direct" => Ok(Self::Direct),
            "confirm" => Ok(Self::Confirm),
            "whitelist" => Ok(Self::Whitelist),
            other => Err(format!(
                "unknown security mode: {other} (valid: direct, confirm, whitelist)"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityDecision {
    Allow,
    Deny(String),
}

#[async_trait]
pub trait UserConfirmation: Send + Sync {
    async fn confirm(&self, command: &str) -> bool;
}

pub struct StdioConfirmation;

#[async_trait]
impl UserConfirmation for StdioConfirmation {
    async fn confirm(&self, command: &str) -> bool {
        eprintln!("[security] Execute command? [y/N]: {command}");
        let mut input = String::new();
        match std::io::stdin().read_line(&mut input) {
            Ok(_) => input.trim().to_lowercase() == "y",
            Err(_) => false,
        }
    }
}

#[derive(Clone)]
pub struct SecurityManager {
    mode: SecurityMode,
    whitelist: Vec<String>,
    confirmer: Arc<dyn UserConfirmation>,
}

impl SecurityManager {
    pub fn new(mode: SecurityMode) -> Self {
        Self {
            mode,
            whitelist: Vec::new(),
            confirmer: Arc::new(StdioConfirmation),
        }
    }

    pub fn with_whitelist(mut self, whitelist: Vec<String>) -> Self {
        self.whitelist = whitelist;
        self
    }

    pub fn with_confirmer(mut self, confirmer: Arc<dyn UserConfirmation>) -> Self {
        self.confirmer = confirmer;
        self
    }

    pub fn from_config(config: &SecurityConfig) -> Self {
        let mode = config.mode.parse().unwrap_or_default();
        Self {
            mode,
            whitelist: config.whitelist.clone(),
            confirmer: Arc::new(StdioConfirmation),
        }
    }

    pub fn from_config_with_override(config: &SecurityConfig, cli_mode: Option<SecurityMode>) -> Self {
        let mode = cli_mode.unwrap_or_else(|| config.mode.parse().unwrap_or_default());
        Self {
            mode,
            whitelist: config.whitelist.clone(),
            confirmer: Arc::new(StdioConfirmation),
        }
    }

    pub fn mode(&self) -> SecurityMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: SecurityMode) {
        self.mode = mode;
    }

    fn extract_command(args: &serde_json::Value) -> Option<String> {
        args.get("command").and_then(|v| v.as_str()).map(String::from)
    }

    pub fn check(&self, args: &serde_json::Value) -> SecurityDecision {
        match self.mode {
            SecurityMode::Direct => SecurityDecision::Allow,
            SecurityMode::Confirm => SecurityDecision::Allow,
            SecurityMode::Whitelist => {
                let command = match Self::extract_command(args) {
                    Some(cmd) => cmd,
                    None => {
                        return SecurityDecision::Deny("no command found in args".into())
                    }
                };
                whitelist::check_whitelist(&command, &self.whitelist)
            }
        }
    }

    pub async fn execute(
        &self,
        tool: &dyn Tool,
        args: serde_json::Value,
    ) -> anyhow::Result<ToolResult> {
        match self.mode {
            SecurityMode::Direct => direct::execute(tool, args).await,
            SecurityMode::Confirm => confirm::execute(tool, args, self.confirmer.as_ref()).await,
            SecurityMode::Whitelist => whitelist::execute(tool, args, &self.whitelist).await,
        }
    }

    pub fn wrap(&self, tool: Box<dyn Tool>) -> SecureTool {
        SecureTool {
            inner: tool,
            manager: Arc::new(self.clone()),
        }
    }
}

pub struct SecureTool {
    inner: Box<dyn Tool>,
    manager: Arc<SecurityManager>,
}

impl SecureTool {
    pub fn new(tool: Box<dyn Tool>, manager: Arc<SecurityManager>) -> Self {
        Self { inner: tool, manager }
    }

    pub fn inner(&self) -> &dyn Tool {
        self.inner.as_ref()
    }
}

#[async_trait]
impl Tool for SecureTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        self.manager.execute(self.inner.as_ref(), args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_display_roundtrip() {
        assert_eq!(SecurityMode::Direct.to_string(), "direct");
        assert_eq!(SecurityMode::Confirm.to_string(), "confirm");
        assert_eq!(SecurityMode::Whitelist.to_string(), "whitelist");
    }

    #[test]
    fn mode_from_str_valid() {
        assert_eq!("direct".parse::<SecurityMode>().unwrap(), SecurityMode::Direct);
        assert_eq!("confirm".parse::<SecurityMode>().unwrap(), SecurityMode::Confirm);
        assert_eq!("whitelist".parse::<SecurityMode>().unwrap(), SecurityMode::Whitelist);
        assert_eq!("DIRECT".parse::<SecurityMode>().unwrap(), SecurityMode::Direct);
    }

    #[test]
    fn mode_from_str_invalid() {
        assert!("bogus".parse::<SecurityMode>().is_err());
    }

    #[test]
    fn check_direct_allows_everything() {
        let mgr = SecurityManager::new(SecurityMode::Direct);
        let args = serde_json::json!({"command": "rm -rf /"});
        assert_eq!(mgr.check(&args), SecurityDecision::Allow);
    }

    #[test]
    fn check_confirm_allows_everything() {
        let mgr = SecurityManager::new(SecurityMode::Confirm);
        let args = serde_json::json!({"command": "rm -rf /"});
        assert_eq!(mgr.check(&args), SecurityDecision::Allow);
    }

    #[test]
    fn check_whitelist_denies_missing_command() {
        let mgr = SecurityManager::new(SecurityMode::Whitelist).with_whitelist(vec!["ls".into()]);
        let args = serde_json::json!({});
        assert!(matches!(mgr.check(&args), SecurityDecision::Deny(_)));
    }

    #[test]
    fn check_whitelist_allows_matching() {
        let mgr = SecurityManager::new(SecurityMode::Whitelist)
            .with_whitelist(vec!["ls".into(), "cat *".into()]);
        assert_eq!(
            mgr.check(&serde_json::json!({"command": "ls"})),
            SecurityDecision::Allow
        );
        assert_eq!(
            mgr.check(&serde_json::json!({"command": "cat /etc/hosts"})),
            SecurityDecision::Allow
        );
    }

    #[test]
    fn check_whitelist_denies_non_matching() {
        let mgr = SecurityManager::new(SecurityMode::Whitelist)
            .with_whitelist(vec!["ls".into()]);
        let args = serde_json::json!({"command": "rm -rf /"});
        assert!(matches!(mgr.check(&args), SecurityDecision::Deny(_)));
    }

    #[test]
    fn from_config_parses_mode() {
        let config = SecurityConfig {
            mode: "whitelist".into(),
            whitelist: vec!["ls".into()],
            ..Default::default()
        };
        let mgr = SecurityManager::from_config(&config);
        assert_eq!(mgr.mode(), SecurityMode::Whitelist);
        assert_eq!(mgr.whitelist, vec!["ls".to_string()]);
    }

    #[test]
    fn from_config_with_override() {
        let config = SecurityConfig {
            mode: "direct".into(),
            whitelist: vec![],
            ..Default::default()
        };
        let mgr = SecurityManager::from_config_with_override(&config, Some(SecurityMode::Confirm));
        assert_eq!(mgr.mode(), SecurityMode::Confirm);
    }

    #[test]
    fn from_config_override_none_uses_config() {
        let config = SecurityConfig {
            mode: "whitelist".into(),
            whitelist: vec![],
            ..Default::default()
        };
        let mgr = SecurityManager::from_config_with_override(&config, None);
        assert_eq!(mgr.mode(), SecurityMode::Whitelist);
    }

    #[tokio::test]
    async fn direct_mode_executes_immediately() {
        let mgr = SecurityManager::new(SecurityMode::Direct);
        let tool = crate::tools::shell::ShellTool::new();
        let result = mgr
            .execute(&tool, serde_json::json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn whitelist_mode_blocks_denied_command() {
        let mgr = SecurityManager::new(SecurityMode::Whitelist)
            .with_whitelist(vec!["echo *".into()]);
        let tool = crate::tools::shell::ShellTool::new();
        let result = mgr
            .execute(&tool, serde_json::json!({"command": "rm -rf /"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not in whitelist"));
    }

    #[tokio::test]
    async fn whitelist_mode_allows_matching_command() {
        let mgr = SecurityManager::new(SecurityMode::Whitelist)
            .with_whitelist(vec!["echo *".into()]);
        let tool = crate::tools::shell::ShellTool::new();
        let result = mgr
            .execute(&tool, serde_json::json!({"command": "echo safe"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("safe"));
    }

    #[tokio::test]
    async fn secure_tool_delegates_to_inner() {
        let mgr = Arc::new(SecurityManager::new(SecurityMode::Direct));
        let inner: Box<dyn Tool> = Box::new(crate::tools::shell::ShellTool::new());
        let secure = SecureTool::new(inner, mgr);

        assert_eq!(secure.name(), "shell");
        assert!(secure.description().contains("shell command"));
        let result = secure
            .execute(serde_json::json!({"command": "echo wrapped"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("wrapped"));
    }

    #[tokio::test]
    async fn secure_tool_whitelist_blocks() {
        let mgr = Arc::new(
            SecurityManager::new(SecurityMode::Whitelist)
                .with_whitelist(vec!["echo *".into()]),
        );
        let inner: Box<dyn Tool> = Box::new(crate::tools::shell::ShellTool::new());
        let secure = SecureTool::new(inner, mgr);

        let result = secure
            .execute(serde_json::json!({"command": "rm -rf /"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn confirm_mode_denies_when_user_rejects() {
        struct DenyAll;
        #[async_trait]
        impl UserConfirmation for DenyAll {
            async fn confirm(&self, _command: &str) -> bool {
                false
            }
        }

        let mgr = SecurityManager::new(SecurityMode::Confirm)
            .with_confirmer(Arc::new(DenyAll));
        let tool = crate::tools::shell::ShellTool::new();
        let result = mgr
            .execute(&tool, serde_json::json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("denied by user"));
    }

    #[tokio::test]
    async fn confirm_mode_allows_when_user_accepts() {
        struct AllowAll;
        #[async_trait]
        impl UserConfirmation for AllowAll {
            async fn confirm(&self, _command: &str) -> bool {
                true
            }
        }

        let mgr = SecurityManager::new(SecurityMode::Confirm)
            .with_confirmer(Arc::new(AllowAll));
        let tool = crate::tools::shell::ShellTool::new();
        let result = mgr
            .execute(&tool, serde_json::json!({"command": "echo yes"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("yes"));
    }
}
