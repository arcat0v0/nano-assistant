pub mod whitelist;

use crate::config::schema::SecurityConfig;
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
    async fn confirm(&self, action: &str) -> bool;
}

pub struct StdioConfirmation;

#[async_trait]
impl UserConfirmation for StdioConfirmation {
    async fn confirm(&self, action: &str) -> bool {
        eprintln!("[security] Execute tool? [y/N]: {action}");
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

    pub fn from_config_with_override(
        config: &SecurityConfig,
        cli_mode: Option<SecurityMode>,
    ) -> Self {
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

    fn extract_command(args: &serde_json::Value) -> Option<&str> {
        args.get("command").and_then(|v| v.as_str())
    }

    pub async fn authorize(&self, tool_name: &str, args: &serde_json::Value) -> Result<(), String> {
        match self.mode {
            SecurityMode::Direct => Ok(()),
            SecurityMode::Confirm => {
                let action = match Self::extract_command(args) {
                    Some(command) => format!("{tool_name}: {command}"),
                    None => format!("{tool_name}: {args}"),
                };
                if self.confirmer.confirm(&action).await {
                    Ok(())
                } else {
                    Err("Execution denied by user".into())
                }
            }
            SecurityMode::Whitelist => {
                let command = Self::extract_command(args)
                    .ok_or_else(|| "no command found in args".to_string())?;
                match whitelist::check_whitelist(command, &self.whitelist) {
                    SecurityDecision::Allow => Ok(()),
                    SecurityDecision::Deny(reason) => Err(reason),
                }
            }
        }
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
        assert_eq!(
            "direct".parse::<SecurityMode>().unwrap(),
            SecurityMode::Direct
        );
        assert_eq!(
            "confirm".parse::<SecurityMode>().unwrap(),
            SecurityMode::Confirm
        );
        assert_eq!(
            "whitelist".parse::<SecurityMode>().unwrap(),
            SecurityMode::Whitelist
        );
        assert_eq!(
            "DIRECT".parse::<SecurityMode>().unwrap(),
            SecurityMode::Direct
        );
    }

    #[test]
    fn mode_from_str_invalid() {
        assert!("bogus".parse::<SecurityMode>().is_err());
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

    #[tokio::test]
    async fn authorization_respects_cli_override_and_whitelist() {
        let config = SecurityConfig {
            mode: "direct".into(),
            whitelist: vec!["echo *".into()],
            ..Default::default()
        };
        let mgr =
            SecurityManager::from_config_with_override(&config, Some(SecurityMode::Whitelist));
        assert!(mgr
            .authorize("shell", &serde_json::json!({"command": "echo safe"}))
            .await
            .is_ok());
        assert!(mgr
            .authorize("shell", &serde_json::json!({"command": "rm -rf /"}))
            .await
            .unwrap_err()
            .contains("not in whitelist"));
        assert!(mgr
            .authorize("file_read", &serde_json::json!({"path": "file.txt"}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn confirmation_denial_prevents_execution() {
        struct DenyAll;
        #[async_trait]
        impl UserConfirmation for DenyAll {
            async fn confirm(&self, _command: &str) -> bool {
                false
            }
        }
        let mgr = SecurityManager::new(SecurityMode::Confirm).with_confirmer(Arc::new(DenyAll));
        assert_eq!(
            mgr.authorize("shell", &serde_json::json!({"command": "echo hello"}))
                .await,
            Err("Execution denied by user".into())
        );
    }
}
