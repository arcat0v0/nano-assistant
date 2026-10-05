pub(crate) mod review;
pub mod whitelist;

use crate::config::schema::SecurityConfig;
use async_trait::async_trait;
use parking_lot::RwLock;
use review::{ReviewDecision, ReviewRequest, SafetyReviewer};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedAction<'a> {
    Shell {
        command: &'a str,
        shell: &'a str,
        flag: &'a str,
    },
    HttpGet {
        url: &'a str,
    },
}

#[derive(serde::Serialize)]
pub struct ToolAction<'a> {
    pub tool_name: &'a str,
    pub args: &'a serde_json::Value,
    pub description: Option<&'a str>,
    pub resolved: Option<ResolvedAction<'a>>,
}

#[derive(Default)]
struct AutoReviewState {
    reviewer: RwLock<Option<Arc<dyn SafetyReviewer>>>,
    user_request: RwLock<Arc<str>>,
    prepared_tools: RwLock<HashSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityMode {
    Direct,
    Confirm,
    Whitelist,
    #[default]
    Auto,
}

impl std::fmt::Display for SecurityMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "direct"),
            Self::Confirm => write!(f, "confirm"),
            Self::Whitelist => write!(f, "whitelist"),
            Self::Auto => write!(f, "auto"),
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
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "unknown security mode: {other} (valid: direct, confirm, whitelist, auto)"
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
    auto: Option<Arc<AutoReviewState>>,
}

impl SecurityManager {
    pub fn new(mode: SecurityMode) -> Self {
        Self {
            mode,
            whitelist: Vec::new(),
            confirmer: Arc::new(StdioConfirmation),
            auto: (mode == SecurityMode::Auto).then(|| Arc::new(AutoReviewState::default())),
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

    pub fn from_config(config: &SecurityConfig) -> Result<Self, String> {
        Self::from_config_with_override(config, None)
    }

    pub fn from_config_with_override(
        config: &SecurityConfig,
        cli_mode: Option<SecurityMode>,
    ) -> Result<Self, String> {
        let mode = match cli_mode {
            Some(mode) => mode,
            None => config.mode.parse()?,
        };
        Ok(Self::new(mode).with_whitelist(config.whitelist.clone()))
    }

    pub fn mode(&self) -> SecurityMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: SecurityMode) {
        self.mode = mode;
        if mode == SecurityMode::Auto && self.auto.is_none() {
            self.auto = Some(Arc::new(AutoReviewState::default()));
        }
    }

    pub(crate) fn with_reviewer(self, reviewer: Arc<dyn SafetyReviewer>) -> Self {
        self.install_reviewer(reviewer);
        self
    }

    pub(crate) fn needs_reviewer(&self) -> bool {
        self.mode == SecurityMode::Auto
            && self
                .auto
                .as_ref()
                .is_some_and(|auto| auto.reviewer.read().is_none())
    }

    pub(crate) fn install_reviewer(&self, reviewer: Arc<dyn SafetyReviewer>) {
        if let Some(auto) = &self.auto {
            let mut installed = auto.reviewer.write();
            if installed.is_none() {
                *installed = Some(reviewer);
            }
        }
    }

    pub(crate) fn set_user_request(&self, request: &str) {
        if let Some(auto) = &self.auto {
            *auto.user_request.write() = Arc::from(request);
        }
    }

    pub(crate) fn register_prepared_tool(&self, name: &str) {
        if self.mode == SecurityMode::Auto {
            if let Some(auto) = &self.auto {
                auto.prepared_tools.write().insert(name.to_owned());
            }
        }
    }

    pub(crate) fn reviews_in_executor(&self, name: &str) -> bool {
        self.mode == SecurityMode::Auto
            && self
                .auto
                .as_ref()
                .is_some_and(|auto| auto.prepared_tools.read().contains(name))
    }

    async fn authorize_auto(&self, action: &ToolAction<'_>) -> Result<(), String> {
        let auto = self.auto.as_ref().expect("auto mode has state");
        let reviewer = auto.reviewer.read().clone();
        let user_request = Arc::clone(&auto.user_request.read());
        let cwd = std::env::current_dir();
        let status = match (reviewer, user_request.is_empty(), cwd) {
            (Some(reviewer), false, Ok(cwd)) => {
                let request = ReviewRequest {
                    user_request: &user_request,
                    action,
                    cwd: &cwd,
                    platform: std::env::consts::OS,
                };
                match reviewer.review(&request).await {
                    Ok(outcome) if outcome.decision == ReviewDecision::Safe => {
                        eprintln!("[security] Safety review: automatically approved");
                        return Ok(());
                    }
                    Ok(outcome) => format!(
                        "{}: {}",
                        match outcome.decision {
                            ReviewDecision::Safe => "safe",
                            ReviewDecision::Risky => "risky",
                            ReviewDecision::Unknown => "unknown",
                        },
                        serde_json::to_string(&outcome.reason).expect("string serialization")
                    ),
                    Err(error) => error.to_string(),
                }
            }
            (None, _, _) => "MissingReviewer".to_owned(),
            (_, true, _) => "MissingUserRequest".to_owned(),
            (_, _, Err(_)) => "UnavailableCwd".to_owned(),
        };
        eprintln!("[security] Safety review: waiting for human confirmation");
        let prompt = escape_terminal_controls(format!(
            "{}; safety review: {status}",
            serde_json::to_string(action).map_err(|_| "Unable to serialize safety action")?
        ));
        if self.confirmer.confirm(&prompt).await {
            Ok(())
        } else {
            Err("Execution denied by user after safety review".into())
        }
    }
    fn extract_command(args: &serde_json::Value) -> Option<&str> {
        args.get("command").and_then(|v| v.as_str())
    }

    pub async fn authorize(&self, action: &ToolAction<'_>) -> Result<(), String> {
        let tool_name = action.tool_name;
        let args = action.args;
        match self.mode {
            SecurityMode::Direct => Ok(()),
            SecurityMode::Auto => self.authorize_auto(action).await,
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

fn escape_terminal_controls(text: String) -> String {
    use std::fmt::Write;
    let extra: usize = text
        .chars()
        .filter(|ch| ch.is_control())
        .map(|ch| 6 - ch.len_utf8())
        .sum();
    if extra == 0 {
        return text;
    }
    let mut escaped = String::with_capacity(text.len() + extra);
    for ch in text.chars() {
        if ch.is_control() {
            write!(escaped, "\\u{:04x}", ch as u32).expect("string formatting");
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedReviewer(Result<ReviewDecision, review::ReviewError>);
    #[async_trait]
    impl SafetyReviewer for FixedReviewer {
        async fn review(
            &self,
            _: &ReviewRequest<'_>,
        ) -> Result<review::ReviewOutcome, review::ReviewError> {
            self.0.map(|decision| review::ReviewOutcome {
                decision,
                reason: "bounded action \u{1b}[31m\u{9b}31m".into(),
            })
        }
    }

    struct ConfirmSequence(parking_lot::Mutex<std::collections::VecDeque<bool>>);
    #[async_trait]
    impl UserConfirmation for ConfirmSequence {
        async fn confirm(&self, action: &str) -> bool {
            assert!(!action.chars().any(char::is_control));
            self.0.lock().pop_front().expect("unexpected confirmation")
        }
    }

    #[tokio::test]
    async fn auto_safe_bypasses_confirmation_others_require_current_approval() {
        let args = serde_json::json!({"command": "echo \u{1b}[31m\u{9b}31m"});
        let action = ToolAction {
            tool_name: "shell",
            args: &args,
            description: None,
            resolved: None,
        };
        for decision in [
            Ok(ReviewDecision::Safe),
            Ok(ReviewDecision::Risky),
            Ok(ReviewDecision::Unknown),
            Err(review::ReviewError::Timeout),
            Err(review::ReviewError::RequestFailed),
            Err(review::ReviewError::InvalidResponse),
        ] {
            let safe = decision == Ok(ReviewDecision::Safe);
            let approvals = if safe { vec![] } else { vec![true, false] };
            let manager = SecurityManager::new(SecurityMode::Auto)
                .with_reviewer(Arc::new(FixedReviewer(decision)))
                .with_confirmer(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                    approvals.into(),
                ))));
            manager.set_user_request("echo text");
            assert!(manager.authorize(&action).await.is_ok());
            assert_eq!(manager.authorize(&action).await.is_ok(), safe);
        }
    }

    #[tokio::test]
    async fn auto_missing_context_or_reviewer_requires_confirmation() {
        let args = serde_json::json!({});
        let action = ToolAction {
            tool_name: "shell",
            args: &args,
            description: None,
            resolved: None,
        };
        let manager = SecurityManager::new(SecurityMode::Auto).with_confirmer(Arc::new(
            ConfirmSequence(parking_lot::Mutex::new(vec![false].into())),
        ));
        assert!(manager.authorize(&action).await.is_err());
        let manager = SecurityManager::new(SecurityMode::Auto)
            .with_reviewer(Arc::new(FixedReviewer(Ok(ReviewDecision::Safe))))
            .with_confirmer(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                vec![false].into(),
            ))));
        assert!(manager.authorize(&action).await.is_err());
    }

    #[test]
    fn auto_mode_and_invalid_config_fail_closed() {
        assert_eq!("auto".parse::<SecurityMode>().unwrap(), SecurityMode::Auto);
        assert_eq!(SecurityMode::Auto.to_string(), "auto");
        let config = SecurityConfig {
            mode: "invalid".into(),
            ..Default::default()
        };
        assert!(SecurityManager::from_config(&config).is_err());
    }

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
        let mgr = SecurityManager::from_config(&config).unwrap();
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
            SecurityManager::from_config_with_override(&config, Some(SecurityMode::Whitelist))
                .unwrap();
        assert!(mgr
            .authorize(&ToolAction {
                tool_name: "shell",
                args: &serde_json::json!({"command": "echo safe"}),
                description: None,
                resolved: None
            })
            .await
            .is_ok());
        assert!(mgr
            .authorize(&ToolAction {
                tool_name: "shell",
                args: &serde_json::json!({"command": "rm -rf /"}),
                description: None,
                resolved: None
            })
            .await
            .unwrap_err()
            .contains("not in whitelist"));
        assert!(mgr
            .authorize(&ToolAction {
                tool_name: "file_read",
                args: &serde_json::json!({"path": "file.txt"}),
                description: None,
                resolved: None
            })
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
            mgr.authorize(&ToolAction {
                tool_name: "shell",
                args: &serde_json::json!({"command": "echo hello"}),
                description: None,
                resolved: None
            })
            .await,
            Err("Execution denied by user".into())
        );
    }
}
