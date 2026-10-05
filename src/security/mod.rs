pub(crate) mod review;
pub mod whitelist;

use crate::config::schema::SecurityConfig;
use async_trait::async_trait;
use parking_lot::RwLock;
use review::{ActionRecord, ReviewDecision, ReviewRequest, SafetyReviewer};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
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
    FileMutation {
        evidence: &'a serde_json::Value,
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
    prepared_tools: RwLock<HashSet<String>>,
    context: RwLock<ReviewContext>,
    review_gate: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct ReviewContext {
    user_request: Arc<str>,
    generation: u64,
    denied: VecDeque<(String, String)>,
    history: VecDeque<ActionRecord>,
}

impl ReviewContext {
    fn record(
        &mut self,
        action: &ToolAction<'_>,
        fingerprint: &str,
        receipt: &str,
        status: &str,
        reason: &str,
    ) {
        let mut value = serde_json::to_value(action).expect("serializable tool action");
        let mut truncated = false;
        if value.to_string().len() > 4096 {
            value = serde_json::json!({"tool_name": action.tool_name, "arguments_omitted": true});
            truncated = true;
        }
        self.history.push_back(ActionRecord {
            action: value,
            fingerprint: fingerprint.to_owned(),
            receipt: receipt.to_owned(),
            status: status.to_owned(),
            reason: reason.chars().take(512).collect(),
            truncated,
        });
        while self.history.len() > 20
            || serde_json::to_vec(&self.history)
                .expect("serializable history")
                .len()
                > 32 * 1024
        {
            self.history.pop_front();
            if let Some(first) = self.history.front_mut() {
                first.truncated = true;
            }
        }
    }

    fn deny(&mut self, fingerprint: String, reason: String) {
        self.denied.push_back((fingerprint, reason));
        if self.denied.len() > 32 {
            self.denied.pop_front();
        }
    }
}

#[derive(Clone)]
pub(crate) struct ReviewReceipt {
    generation: u64,
    id: String,
}

fn action_fingerprint(action: &ToolAction<'_>) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(action).expect("serializable action"))
    )
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
        eprintln!(
            "  {}  {} {action}",
            crate::console::dim_label("│"),
            crate::console::confirm_prompt()
        );
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
            let mut context = auto.context.write();
            let generation = context.generation.wrapping_add(1);
            *context = ReviewContext {
                user_request: Arc::from(request),
                generation,
                ..Default::default()
            };
        }
    }

    pub(crate) fn register_prepared_tool(&self, name: &str) {
        if self.mode == SecurityMode::Auto {
            if let Some(auto) = &self.auto {
                auto.prepared_tools.write().insert(name.to_owned());
            }
        }
    }

    pub(crate) fn unregister_prepared_tool(&self, name: &str) {
        if let Some(auto) = &self.auto {
            auto.prepared_tools.write().remove(name);
        }
    }

    pub(crate) fn record_execution(&self, receipt: Option<&ReviewReceipt>, succeeded: bool) {
        if let (Some(auto), Some(receipt)) = (&self.auto, receipt) {
            let mut context = auto.context.write();
            if context.generation != receipt.generation {
                return;
            }
            if let Some(record) = context
                .history
                .iter_mut()
                .find(|record| record.receipt == receipt.id)
            {
                record.status = if succeeded { "executed" } else { "failed" }.into();
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
        self.authorize_auto_with_preview(action, None)
            .await
            .map(|_| ())
    }

    pub(crate) async fn authorize_prepared(
        &self,
        action: &ToolAction<'_>,
        preview: &str,
    ) -> Result<Option<ReviewReceipt>, String> {
        if self.mode == SecurityMode::Auto {
            self.authorize_auto_with_preview(action, Some(preview))
                .await
                .map(Some)
        } else {
            self.authorize(action).await.map(|_| None)
        }
    }

    pub(crate) async fn authorize_execution(
        &self,
        action: &ToolAction<'_>,
    ) -> Result<Option<ReviewReceipt>, String> {
        if self.mode == SecurityMode::Auto {
            self.authorize_auto_with_preview(action, None)
                .await
                .map(Some)
        } else {
            self.authorize(action).await.map(|_| None)
        }
    }

    async fn authorize_auto_with_preview(
        &self,
        action: &ToolAction<'_>,
        preview: Option<&str>,
    ) -> Result<ReviewReceipt, String> {
        let auto = self.auto.as_ref().expect("auto mode has state");
        let _gate = auto.review_gate.lock().await;
        let fingerprint = action_fingerprint(action);
        let (user_request, generation, history) = {
            let context = auto.context.read();
            if let Some((_, reason)) = context.denied.iter().find(|(key, _)| key == &fingerprint) {
                return Err(format!("{reason} Unchanged action and evidence were already rejected; do not resubmit or bypass using another tool."));
            }
            (
                Arc::clone(&context.user_request),
                context.generation,
                context.history.iter().cloned().collect::<Vec<_>>(),
            )
        };
        let reviewer = auto.reviewer.read().clone();
        let cwd = std::env::current_dir();
        let (mut decision, mut detail) = match (reviewer, user_request.is_empty(), cwd) {
            (Some(reviewer), false, Ok(cwd)) => {
                let request = ReviewRequest {
                    user_request: &user_request,
                    action,
                    cwd: &cwd,
                    platform: std::env::consts::OS,
                    history: &history,
                };
                match reviewer.review(&request).await {
                    Ok(outcome) => {
                        let mut detail = outcome.reason.clone();
                        if !outcome.missing_evidence.is_empty() {
                            detail.push_str(&format!(
                                " · missing evidence: {}",
                                outcome.missing_evidence.join("; ")
                            ));
                        }
                        (outcome.decision(), detail)
                    }
                    Err(error) => (
                        ReviewDecision::AskUser,
                        format!("Safety review unavailable: {error}"),
                    ),
                }
            }
            (None, _, _) => (ReviewDecision::AskUser, "MissingReviewer".into()),
            (_, true, _) => (ReviewDecision::AskUser, "MissingUserRequest".into()),
            (_, _, Err(_)) => (ReviewDecision::AskUser, "UnavailableCwd".into()),
        };
        if let Some(ResolvedAction::FileMutation { evidence }) = &action.resolved {
            if evidence["protected_backup"] == true {
                decision = ReviewDecision::Deny;
                detail = "Existing files under ~/Backup are protected by safety policy".into();
            }
        }
        if auto.context.read().generation != generation {
            return Err("Execution denied: user request changed during safety review".into());
        }
        let (result, status) = match decision {
            ReviewDecision::Allow => {
                eprintln!(
                    "{}",
                    crate::console::review_note(&crate::console::green("allowed"))
                );
                (Ok(()), "approved")
            }
            ReviewDecision::Deny => {
                eprintln!(
                    "{}",
                    crate::console::review_note(&format!(
                        "{} · {}",
                        crate::console::red("denied"),
                        escape_terminal_controls(detail.clone())
                    ))
                );
                (Err(format!("Execution denied by safety policy: {detail}. Do not repeat or bypass this action; explain the reason and only submit a materially changed action that resolves the concern.")), "denied")
            }
            ReviewDecision::AskUser => {
                eprintln!(
                    "{}",
                    crate::console::review_note(&format!(
                        "{} · {}",
                        crate::console::yellow("waiting for human confirmation"),
                        escape_terminal_controls(detail.clone())
                    ))
                );
                let serialized = confirmation_action(action);
                let prompt = escape_terminal_controls(format!(
                    "{serialized}; safety review: {detail}{}",
                    preview
                        .map(|p| format!("; local changes: {p}"))
                        .unwrap_or_default()
                ));
                if self.confirmer.confirm(&prompt).await {
                    (Ok(()), "approved")
                } else {
                    (Err(format!("Execution denied by user after safety review: {detail}. Do not repeat this unchanged action or bypass review.")), "user_rejected")
                }
            }
        };
        let mut context = auto.context.write();
        if context.generation != generation {
            return Err("Execution denied: user request changed during confirmation".into());
        }
        let receipt = ReviewReceipt {
            generation,
            id: uuid::Uuid::new_v4().to_string(),
        };
        context.record(action, &fingerprint, &receipt.id, status, &detail);
        if let Err(reason) = &result {
            context.deny(fingerprint, reason.clone());
        }
        result.map(|_| receipt)
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

fn confirmation_action(action: &ToolAction<'_>) -> String {
    let mut display = format!("{} · {}", action.tool_name, action.args);
    match &action.resolved {
        Some(ResolvedAction::Shell {
            command,
            shell,
            flag,
        }) => {
            display.push_str(&format!("; executes {shell} {flag}: {command}"));
        }
        Some(ResolvedAction::HttpGet { url }) => {
            display.push_str(&format!("; GET {url}"));
        }
        Some(ResolvedAction::FileMutation { evidence }) => {
            display.push_str(&format!(
                "; {} {}",
                evidence["operation"], evidence["resolved_path"]
            ));
        }
        None => {}
    }
    display
}

fn escape_terminal_controls(text: String) -> String {
    use std::fmt::Write;
    if !text.chars().any(|ch| ch.is_control()) {
        return text;
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                write!(escaped, "\\u{:04x}", ch as u32).expect("string formatting");
            }
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedReviewer(review::ReviewOutcome, std::sync::atomic::AtomicUsize);
    #[async_trait]
    impl SafetyReviewer for FixedReviewer {
        async fn review(
            &self,
            _: &ReviewRequest<'_>,
        ) -> Result<review::ReviewOutcome, review::ReviewError> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.0.clone())
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

    fn fixed(risk: review::Risk, authorization: review::Authorization) -> Arc<FixedReviewer> {
        Arc::new(FixedReviewer(
            review::ReviewOutcome {
                risk,
                authorization,
                reason: "bounded action \u{1b}[31m".into(),
                missing_evidence: vec![],
            },
            std::sync::atomic::AtomicUsize::new(0),
        ))
    }

    #[tokio::test]
    async fn uncertainty_confirms_once_and_unchanged_rejection_is_cached() {
        let reviewer = fixed(review::Risk::Unknown, review::Authorization::WithinScope);
        let manager = SecurityManager::new(SecurityMode::Auto)
            .with_reviewer(reviewer.clone())
            .with_confirmer(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                vec![false, true].into(),
            ))));
        manager.set_user_request("first");
        let args = serde_json::json!({"command":"echo hi"});
        let action = ToolAction {
            tool_name: "shell",
            args: &args,
            description: None,
            resolved: None,
        };
        assert!(manager.authorize(&action).await.is_err());
        assert!(manager.authorize(&action).await.is_err());
        assert_eq!(reviewer.1.load(std::sync::atomic::Ordering::SeqCst), 1);
        let other = serde_json::json!({"command":"echo revised"});
        assert!(manager
            .authorize(&ToolAction {
                tool_name: "shell",
                args: &other,
                description: None,
                resolved: None
            })
            .await
            .is_ok());
        assert!(manager.authorize(&action).await.is_err());
        assert_eq!(reviewer.1.load(std::sync::atomic::Ordering::SeqCst), 2);
        manager.set_user_request("second");
        manager
            .with_confirmer(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                vec![true].into(),
            ))))
            .authorize(&action)
            .await
            .unwrap();
        assert_eq!(reviewer.1.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn prohibited_or_outside_scope_never_prompt() {
        let args = serde_json::json!({"command":"echo hi"});
        for (risk, authorization) in [
            (
                review::Risk::Prohibited,
                review::Authorization::ExplicitlyApproved,
            ),
            (review::Risk::Low, review::Authorization::OutsideScope),
        ] {
            let manager = SecurityManager::new(SecurityMode::Auto)
                .with_reviewer(fixed(risk, authorization))
                .with_confirmer(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                    vec![].into(),
                ))));
            manager.set_user_request("first");
            assert!(manager
                .authorize(&ToolAction {
                    tool_name: "shell",
                    args: &args,
                    description: None,
                    resolved: None
                })
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn execution_receipts_correlate_identical_parallel_actions_and_ignore_old_turns() {
        let manager = SecurityManager::new(SecurityMode::Auto)
            .with_reviewer(fixed(review::Risk::Low, review::Authorization::WithinScope));
        manager.set_user_request("first");
        let args = serde_json::json!({"command":"echo hi"});
        let action = ToolAction {
            tool_name: "shell",
            args: &args,
            description: None,
            resolved: None,
        };
        let (one, two) = tokio::join!(
            manager.authorize_execution(&action),
            manager.authorize_execution(&action)
        );
        let one = one.unwrap();
        let two = two.unwrap();
        manager.record_execution(two.as_ref(), false);
        manager.record_execution(one.as_ref(), true);
        let auto = manager.auto.as_ref().unwrap();
        assert_eq!(auto.context.read().history[0].status, "executed");
        assert_eq!(auto.context.read().history[1].status, "failed");
        manager.set_user_request("second");
        manager.record_execution(one.as_ref(), true);
        assert!(auto.context.read().history.is_empty());
    }

    #[test]
    fn history_is_bounded_and_marks_omitted_arguments_and_evicted_actions() {
        let mut context = ReviewContext::default();
        let args = serde_json::json!({"content": "x".repeat(5000)});
        let action = ToolAction {
            tool_name: "file_write",
            args: &args,
            description: None,
            resolved: None,
        };
        for i in 0..40 {
            context.record(&action, "fingerprint", &i.to_string(), "denied", "reason");
        }
        assert_eq!(context.history.len(), 20);
        assert!(context.history.iter().all(|record| record.truncated));
        assert!(serde_json::to_vec(&context.history).unwrap().len() <= 32 * 1024);
        assert!(!serde_json::to_string(&context.history)
            .unwrap()
            .contains(&"x".repeat(5000)));
    }

    #[tokio::test]
    async fn changed_request_cancels_in_flight_review_without_prompting() {
        struct PausedReviewer {
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }
        #[async_trait]
        impl SafetyReviewer for PausedReviewer {
            async fn review(
                &self,
                _: &ReviewRequest<'_>,
            ) -> Result<review::ReviewOutcome, review::ReviewError> {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(review::ReviewOutcome {
                    risk: review::Risk::Unknown,
                    authorization: review::Authorization::WithinScope,
                    reason: "uncertain".into(),
                    missing_evidence: vec![],
                })
            }
        }
        let reviewer = Arc::new(PausedReviewer {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let manager = Arc::new(
            SecurityManager::new(SecurityMode::Auto)
                .with_reviewer(reviewer.clone())
                .with_confirmer(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                    vec![].into(),
                )))),
        );
        manager.set_user_request("first");
        let executing = manager.clone();
        let task = tokio::spawn(async move {
            let args = serde_json::json!({"command":"echo hi"});
            executing
                .authorize(&ToolAction {
                    tool_name: "shell",
                    args: &args,
                    description: None,
                    resolved: None,
                })
                .await
        });
        reviewer.entered.notified().await;
        manager.set_user_request("second");
        reviewer.release.notify_one();
        assert!(task.await.unwrap().unwrap_err().contains("request changed"));
        assert!(manager
            .auto
            .as_ref()
            .unwrap()
            .context
            .read()
            .history
            .is_empty());
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
            .with_reviewer(fixed(review::Risk::Low, review::Authorization::WithinScope))
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
