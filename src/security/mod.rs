pub(crate) mod evidence;
pub(crate) mod review;
pub mod whitelist;

use crate::config::schema::SecurityConfig;
use crate::interaction::{
    AskCancelReason, AskRequest, AskResult, ConfirmationRequest, HumanInteraction,
};
#[cfg(test)]
use async_trait::async_trait;
use parking_lot::RwLock;
use review::{ActionRecord, ReviewDecision, ReviewRequest, SafetyReviewer, UserClarification};
use rig::tool::ToolExecutionError;
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
    review_gate: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct ReviewContext {
    user_request: Arc<str>,
    generation: u64,
    denied: VecDeque<(String, u64, String)>,
    history: VecDeque<ActionRecord>,
    interaction_cancelled: bool,
    successful_ask_batches: usize,
    clarifications: Vec<UserClarification>,
    clarification_revision: u64,
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
        self.denied
            .push_back((fingerprint, self.clarification_revision, reason));
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

#[derive(Clone)]
pub struct SecurityManager {
    mode: SecurityMode,
    whitelist: Vec<String>,
    interaction: Arc<dyn HumanInteraction>,
    context: Arc<RwLock<ReviewContext>>,
    auto: Option<Arc<AutoReviewState>>,
}

impl SecurityManager {
    pub fn new(mode: SecurityMode) -> Self {
        Self {
            mode,
            whitelist: Vec::new(),
            interaction: Arc::new(crate::tui::interaction::TerminalInteraction::default()),
            context: Arc::new(RwLock::new(ReviewContext::default())),
            auto: (mode == SecurityMode::Auto).then(|| Arc::new(AutoReviewState::default())),
        }
    }

    pub fn with_whitelist(mut self, whitelist: Vec<String>) -> Self {
        self.whitelist = whitelist;
        self
    }

    pub fn with_interaction(mut self, interaction: Arc<dyn HumanInteraction>) -> Self {
        self.interaction = interaction;
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
        let mut context = self.context.write();
        let generation = context.generation.wrapping_add(1);
        *context = ReviewContext {
            user_request: Arc::from(request),
            generation,
            ..Default::default()
        };
    }

    pub(crate) fn interaction_cancelled(&self) -> bool {
        self.context.read().interaction_cancelled
    }

    fn check_interaction(&self) -> Result<(), String> {
        if self.interaction_cancelled() {
            Err(CLARIFICATION_CANCELLED.into())
        } else {
            Ok(())
        }
    }

    pub(crate) async fn ask(&self, request: &AskRequest) -> Result<AskResult, ToolExecutionError> {
        request
            .validate()
            .map_err(ToolExecutionError::invalid_args)?;
        let generation = {
            let context = self.context.read();
            if context.interaction_cancelled {
                return Err(ToolExecutionError::other(CLARIFICATION_CANCELLED));
            }
            let maximum_bytes = request
                .questions
                .iter()
                .map(|q| {
                    serde_json::to_vec(&serde_json::json!({
                        "question_id": q.id, "question": q.question,
                        "selected_options": if q.multi { &q.options[..] } else { &[] },
                        "custom": "",
                    }))
                    .expect("serializable clarification")
                    .len()
                        + 2048 * 6
                        + 1
                })
                .sum::<usize>();
            if context.successful_ask_batches >= 8
                || serde_json::to_vec(&context.clarifications)
                    .expect("serializable clarifications")
                    .len()
                    + maximum_bytes
                    > 64 * 1024
            {
                return Err(ToolExecutionError::invalid_args(
                    "Clarification capacity exceeded",
                ));
            }
            context.generation
        };
        let result = self.interaction.ask(request).await;
        if let AskResult::Answered { answers } = &result {
            request
                .validate_answers(answers)
                .map_err(ToolExecutionError::invalid_args)?;
        }
        let mut context = self.context.write();
        if context.generation != generation {
            return Ok(AskResult::Cancelled {
                reason: AskCancelReason::Unavailable,
            });
        }
        if context.interaction_cancelled {
            return Ok(AskResult::Cancelled {
                reason: AskCancelReason::Unavailable,
            });
        }
        match &result {
            AskResult::Answered { answers } => {
                let additions = answers
                    .iter()
                    .map(|answer| {
                        let question = request
                            .questions
                            .iter()
                            .find(|q| q.id == answer.question_id)
                            .expect("validated answer");
                        UserClarification {
                            question_id: question.id.clone(),
                            question: question.question.clone(),
                            selected_options: question
                                .options
                                .iter()
                                .filter(|o| answer.selected.contains(&o.id))
                                .cloned()
                                .collect(),
                            custom: answer.custom.clone(),
                        }
                    })
                    .collect::<Vec<_>>();
                let existing_bytes = serde_json::to_vec(&context.clarifications)
                    .expect("serializable clarifications")
                    .len();
                let added_bytes = serde_json::to_vec(&additions)
                    .expect("serializable clarifications")
                    .len();
                let combined_bytes = existing_bytes + added_bytes - 2
                    + usize::from(!context.clarifications.is_empty());
                if context.successful_ask_batches >= 8 || combined_bytes > 64 * 1024 {
                    return Err(ToolExecutionError::invalid_args(
                        "Clarification capacity exceeded",
                    ));
                }
                context.clarifications.extend(additions);
                context.successful_ask_batches += 1;
                context.clarification_revision = context.clarification_revision.wrapping_add(1);
            }
            AskResult::Cancelled { .. } => context.interaction_cancelled = true,
        }
        Ok(result)
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
        if let Some(receipt) = receipt {
            let mut context = self.context.write();
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
        self.check_interaction()?;
        if self.mode == SecurityMode::Auto {
            self.authorize_auto_with_preview(action, Some(preview))
                .await
                .map(Some)
        } else if self.mode == SecurityMode::Confirm {
            self.confirm_action(action, Some(preview))
                .await
                .map(|_| None)
        } else {
            self.authorize(action).await.map(|_| None)
        }
    }

    pub(crate) async fn authorize_execution(
        &self,
        action: &ToolAction<'_>,
    ) -> Result<Option<ReviewReceipt>, String> {
        self.check_interaction()?;
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
        let (user_request, generation, revision, history, clarifications) = {
            let context = self.context.read();
            if context.interaction_cancelled {
                return Err(CLARIFICATION_CANCELLED.into());
            }
            if let Some((_, _, reason)) = context.denied.iter().find(|(key, revision, _)| {
                key == &fingerprint && *revision == context.clarification_revision
            }) {
                return Err(format!("{reason} Unchanged action and evidence were already rejected; do not resubmit or bypass using another tool."));
            }
            (
                Arc::clone(&context.user_request),
                context.generation,
                context.clarification_revision,
                context.history.iter().cloned().collect::<Vec<_>>(),
                context.clarifications.clone(),
            )
        };
        let mut missing_evidence = Vec::new();
        let mut risk_label = None;
        let reviewer = auto.reviewer.read().clone();
        let cwd = std::env::current_dir();
        let (mut decision, mut detail) = match (reviewer, user_request.is_empty(), cwd) {
            (Some(reviewer), false, Ok(cwd)) => {
                let request = ReviewRequest {
                    user_request: &user_request,
                    clarifications: &clarifications,
                    action,
                    cwd: &cwd,
                    platform: std::env::consts::OS,
                    history: &history,
                };
                match reviewer.review(&request).await {
                    Ok(outcome) => {
                        missing_evidence = outcome.missing_evidence.clone();
                        risk_label = Some(format!("{:?}", outcome.risk).to_lowercase());
                        (outcome.decision(), outcome.reason)
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
        self.check_review_version(generation, revision, "safety review")?;
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
            ReviewDecision::Investigate => {
                eprintln!(
                    "{}",
                    crate::console::review_note(&format!(
                        "{} · {}",
                        crate::console::yellow("investigation required"),
                        escape_terminal_controls(detail.clone())
                    ))
                );
                let facts = serde_json::to_string(&missing_evidence)
                    .expect("missing evidence strings serialize");
                (
                    Err(format!("Safety review requires investigation: {detail}. Missing facts: {facts}. This action has not executed. Continue investigating with task-scoped tools to resolve these facts. Do not repeat this action without new evidence or bypass safety review; resubmit it for fresh review after investigation.")),
                    "investigation_required",
                )
            }
            ReviewDecision::AskUser => {
                let mut request = confirmation_action(action);
                request.reason = Some(detail.clone());
                request.missing_evidence = missing_evidence;
                request.risk_label = risk_label;
                request.preview = preview.map(str::to_owned);
                if self.interaction.confirm(&request).await {
                    (Ok(()), "approved")
                } else {
                    (Err(format!("Execution denied by user after safety review: {detail}. Do not repeat this unchanged action or bypass review.")), "user_rejected")
                }
            }
        };
        let mut context = self.context.write();
        if context.generation != generation {
            return Err("Execution denied: user request changed during confirmation".into());
        }
        if context.clarification_revision != revision {
            return Err("Execution denied: user clarification changed during safety review".into());
        }
        if context.interaction_cancelled {
            return Err(CLARIFICATION_CANCELLED.into());
        }
        let receipt = ReviewReceipt {
            generation,
            id: uuid::Uuid::new_v4().to_string(),
        };
        context.record(action, &fingerprint, &receipt.id, status, &detail);
        if decision != ReviewDecision::Investigate {
            if let Err(reason) = &result {
                context.deny(fingerprint, reason.clone());
            }
        }
        result.map(|_| receipt)
    }
    fn check_review_version(
        &self,
        generation: u64,
        revision: u64,
        stage: &str,
    ) -> Result<(), String> {
        let context = self.context.read();
        if context.generation != generation {
            return Err(format!(
                "Execution denied: user request changed during {stage}"
            ));
        }
        if context.clarification_revision != revision {
            return Err("Execution denied: user clarification changed during safety review".into());
        }
        if context.interaction_cancelled {
            return Err(CLARIFICATION_CANCELLED.into());
        }
        Ok(())
    }
    fn extract_command(args: &serde_json::Value) -> Option<&str> {
        args.get("command").and_then(|v| v.as_str())
    }

    async fn confirm_action(
        &self,
        action: &ToolAction<'_>,
        preview: Option<&str>,
    ) -> Result<(), String> {
        self.check_interaction()?;
        let (generation, revision) = {
            let context = self.context.read();
            (context.generation, context.clarification_revision)
        };
        let mut request = confirmation_action(action);
        request.preview = preview.map(str::to_owned);
        let allowed = self.interaction.confirm(&request).await;
        self.check_review_version(generation, revision, "confirmation")?;
        if allowed {
            Ok(())
        } else {
            Err("Execution denied by user".into())
        }
    }

    pub async fn authorize(&self, action: &ToolAction<'_>) -> Result<(), String> {
        self.check_interaction()?;
        let args = action.args;
        match self.mode {
            SecurityMode::Direct => Ok(()),
            SecurityMode::Auto => self.authorize_auto(action).await,
            SecurityMode::Confirm => self.confirm_action(action, None).await,
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

const CLARIFICATION_CANCELLED: &str =
    "User cancelled clarification; do not execute further tools in this turn.";

fn confirmation_action(action: &ToolAction<'_>) -> ConfirmationRequest {
    let details = match &action.resolved {
        Some(ResolvedAction::Shell {
            command,
            shell,
            flag,
        }) => {
            let mut details = format!("{shell} {flag}:\n{command}");
            let arguments = match action.args.as_object() {
                Some(args)
                    if args.get("command").and_then(serde_json::Value::as_str)
                        == Some(*command) =>
                {
                    let remaining = args
                        .iter()
                        .filter(|(key, _)| key.as_str() != "command")
                        .collect::<std::collections::BTreeMap<_, _>>();
                    (!remaining.is_empty()).then(|| {
                        serde_json::to_string_pretty(&remaining).expect("serializable args")
                    })
                }
                _ => Some(serde_json::to_string_pretty(action.args).expect("serializable args")),
            };
            if let Some(arguments) = arguments {
                details.push_str(&format!("\nArguments:\n{arguments}"));
            }
            details
        }
        Some(ResolvedAction::HttpGet { url }) => format!("GET {url}"),
        Some(ResolvedAction::FileMutation { evidence }) => format!(
            "{} {}\n{}",
            evidence["operation"],
            evidence["resolved_path"],
            serde_json::to_string_pretty(action.args).expect("serializable args")
        ),
        None => serde_json::to_string_pretty(action.args).expect("serializable args"),
    };
    let summary = match &action.resolved {
        Some(ResolvedAction::Shell { command, .. }) => {
            command.lines().next().unwrap_or(command).to_string()
        }
        Some(ResolvedAction::HttpGet { url }) => format!("GET {url}"),
        Some(ResolvedAction::FileMutation { evidence }) => {
            format!("{} {}", evidence["operation"], evidence["resolved_path"])
        }
        None => action
            .args
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(action.tool_name)
            .to_string(),
    };
    ConfirmationRequest {
        tool_name: action.tool_name.to_owned(),
        summary,
        details,
        reason: None,
        missing_evidence: Vec::new(),
        risk_label: None,
        preview: None,
    }
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

    struct AnswerInteraction(AskResult);
    #[async_trait]
    impl HumanInteraction for AnswerInteraction {
        async fn ask(&self, _: &AskRequest) -> AskResult {
            self.0.clone()
        }
        async fn confirm(&self, _: &ConfirmationRequest) -> bool {
            false
        }
    }

    fn text_request() -> AskRequest {
        serde_json::from_value(
            serde_json::json!({"questions":[{"id":"detail","question":"Which data?"}]}),
        )
        .unwrap()
    }

    fn answered() -> AskResult {
        AskResult::Answered {
            answers: vec![crate::interaction::AskAnswer {
                question_id: "detail".into(),
                selected: vec![],
                custom: Some("keep data".into()),
            }],
        }
    }

    #[tokio::test]
    async fn trusted_ask_is_bounded_and_preserves_history() {
        let manager = SecurityManager::new(SecurityMode::Direct)
            .with_interaction(Arc::new(AnswerInteraction(answered())));
        manager.set_user_request("original");
        for _ in 0..8 {
            manager.ask(&text_request()).await.unwrap();
        }
        assert!(manager.ask(&text_request()).await.is_err());
        let context = manager.context.read();
        assert_eq!(&*context.user_request, "original");
        assert_eq!(context.clarification_revision, 8);
        assert_eq!(context.clarifications.len(), 8);
        assert!(!context.interaction_cancelled);
    }

    #[tokio::test]
    async fn cancelled_ask_blocks_all_modes_and_next_turn_resets() {
        for mode in [
            SecurityMode::Direct,
            SecurityMode::Confirm,
            SecurityMode::Auto,
            SecurityMode::Whitelist,
        ] {
            let manager = SecurityManager::new(mode).with_interaction(Arc::new(AnswerInteraction(
                AskResult::Cancelled {
                    reason: AskCancelReason::Eof,
                },
            )));
            manager.set_user_request("first");
            manager.ask(&text_request()).await.unwrap();
            let args = serde_json::json!({"command":"echo hi"});
            let action = ToolAction {
                tool_name: "shell",
                args: &args,
                description: None,
                resolved: None,
            };
            assert_eq!(
                manager.authorize(&action).await.unwrap_err(),
                CLARIFICATION_CANCELLED
            );
            assert!(manager.ask(&text_request()).await.is_err());
            manager.set_user_request("second");
            assert!(!manager.interaction_cancelled());
            assert!(manager.context.read().clarifications.is_empty());
        }
    }

    #[tokio::test]
    async fn invalid_answers_and_capacity_fail_without_cancelling() {
        let manager = SecurityManager::new(SecurityMode::Direct).with_interaction(Arc::new(
            AnswerInteraction(AskResult::Answered { answers: vec![] }),
        ));
        assert!(manager.ask(&text_request()).await.is_err());
        assert!(!manager.interaction_cancelled());
        manager
            .context
            .write()
            .clarifications
            .push(review::UserClarification {
                question_id: "old".into(),
                question: "x".repeat(64 * 1024),
                selected_options: vec![],
                custom: None,
            });
        assert!(manager.ask(&text_request()).await.is_err());
        assert!(!manager.interaction_cancelled());
    }

    struct PausedInteraction {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait]
    impl HumanInteraction for PausedInteraction {
        async fn ask(&self, _: &AskRequest) -> AskResult {
            self.entered.notify_one();
            self.release.notified().await;
            answered()
        }
        async fn confirm(&self, _: &ConfirmationRequest) -> bool {
            self.entered.notify_one();
            self.release.notified().await;
            true
        }
    }

    #[tokio::test]
    async fn stale_ask_does_not_cancel_or_authorize_new_turn() {
        let interaction = Arc::new(PausedInteraction {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let manager = Arc::new(
            SecurityManager::new(SecurityMode::Direct).with_interaction(interaction.clone()),
        );
        manager.set_user_request("first");
        let asking = manager.clone();
        let task = tokio::spawn(async move { asking.ask(&text_request()).await });
        interaction.entered.notified().await;
        manager.set_user_request("second");
        interaction.release.notify_one();
        assert_eq!(
            task.await.unwrap().unwrap(),
            AskResult::Cancelled {
                reason: AskCancelReason::Unavailable
            }
        );
        assert!(!manager.interaction_cancelled());
        assert!(manager.context.read().clarifications.is_empty());
    }

    #[tokio::test]
    async fn stale_confirmation_cannot_approve_changed_generation_or_revision() {
        for mode in [SecurityMode::Confirm, SecurityMode::Auto] {
            for change_revision in [false, true] {
                let interaction = Arc::new(PausedInteraction {
                    entered: tokio::sync::Notify::new(),
                    release: tokio::sync::Notify::new(),
                });
                let manager = Arc::new(
                    SecurityManager::new(mode)
                        .with_reviewer(fixed(
                            review::Risk::High,
                            review::Authorization::WithinScope,
                        ))
                        .with_interaction(interaction.clone()),
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
                interaction.entered.notified().await;
                if change_revision {
                    let asking = manager
                        .as_ref()
                        .clone()
                        .with_interaction(Arc::new(AnswerInteraction(answered())));
                    asking.ask(&text_request()).await.unwrap();
                } else {
                    manager.set_user_request("second");
                }
                interaction.release.notify_one();
                assert!(task
                    .await
                    .unwrap()
                    .unwrap_err()
                    .contains(if change_revision {
                        "clarification changed"
                    } else {
                        "request changed"
                    }));
                assert!(manager.context.read().history.is_empty());
                assert!(manager.context.read().denied.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn real_clarification_versions_denials_without_erasing_history() {
        let reviewer = fixed(review::Risk::High, review::Authorization::WithinScope);
        let manager = SecurityManager::new(SecurityMode::Auto)
            .with_reviewer(reviewer.clone())
            .with_interaction(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                vec![false, true].into(),
            ))));
        manager.set_user_request("original");
        let args = serde_json::json!({"command":"echo hi"});
        let action = ToolAction {
            tool_name: "shell",
            args: &args,
            description: None,
            resolved: None,
        };
        assert!(manager.authorize(&action).await.is_err());
        assert!(manager.authorize(&action).await.is_err());
        manager.ask(&text_request()).await.unwrap();
        assert_eq!(manager.context.read().history.len(), 1);
        assert_eq!(manager.context.read().denied.len(), 1);
        manager.authorize(&action).await.unwrap();
        assert_eq!(reviewer.1.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(manager.context.read().history.len(), 2);
    }

    #[tokio::test]
    async fn trusted_clarifications_store_only_actual_selected_option_snapshots() {
        let request: AskRequest = serde_json::from_value(serde_json::json!({"questions":[{
            "id":"policy","question":"Handle data?", "options":[
                {"id":"keep","label":"Keep","description":"Keep persistent data"},
                {"id":"delete","label":"Delete","description":"Delete everything"}
            ],"recommended":"delete"
        }]}))
        .unwrap();
        let manager = SecurityManager::new(SecurityMode::Direct).with_interaction(Arc::new(
            AnswerInteraction(AskResult::Answered {
                answers: vec![crate::interaction::AskAnswer {
                    question_id: "policy".into(),
                    selected: vec!["keep".into()],
                    custom: None,
                }],
            }),
        ));
        manager.set_user_request("uninstall");
        manager.ask(&request).await.unwrap();
        let context = manager.context.read();
        assert_eq!(&*context.user_request, "uninstall");
        let selected = &context.clarifications[0].selected_options;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "keep");
        assert_eq!(
            selected[0].description.as_deref(),
            Some("Keep persistent data")
        );
        assert!(!serde_json::to_string(&context.clarifications)
            .unwrap()
            .contains("Delete everything"));
    }

    #[tokio::test]
    async fn clarification_during_review_invalidates_decision_without_caching() {
        struct ClarifyingReviewer(SecurityManager);
        #[async_trait]
        impl SafetyReviewer for ClarifyingReviewer {
            async fn review(
                &self,
                request: &ReviewRequest<'_>,
            ) -> Result<review::ReviewOutcome, review::ReviewError> {
                assert!(request.clarifications.is_empty());
                self.0.ask(&text_request()).await.unwrap();
                Ok(review::ReviewOutcome {
                    risk: review::Risk::Medium,
                    authorization: review::Authorization::WithinScope,
                    reason: "bounded".into(),
                    missing_evidence: vec![],
                })
            }
        }
        let manager = SecurityManager::new(SecurityMode::Auto)
            .with_interaction(Arc::new(AnswerInteraction(answered())));
        manager.set_user_request("original");
        let mut clarifying = SecurityManager::new(SecurityMode::Direct)
            .with_interaction(Arc::new(AnswerInteraction(answered())));
        clarifying.context = Arc::clone(&manager.context);
        manager.install_reviewer(Arc::new(ClarifyingReviewer(clarifying)));
        let args = serde_json::json!({"command":"echo hi"});
        let action = ToolAction {
            tool_name: "shell",
            args: &args,
            description: None,
            resolved: None,
        };
        assert!(manager
            .authorize(&action)
            .await
            .unwrap_err()
            .contains("clarification changed"));
        assert_eq!(manager.context.read().clarification_revision, 1);
        assert!(manager.context.read().denied.is_empty());
        assert!(manager.context.read().history.is_empty());
    }

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
    impl HumanInteraction for ConfirmSequence {
        async fn ask(&self, _: &AskRequest) -> AskResult {
            answered()
        }
        async fn confirm(&self, _: &ConfirmationRequest) -> bool {
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
    async fn runtime_facts_require_investigation_without_confirmation_or_rejection_cache() {
        for (risk, facts) in [
            (
                review::Risk::Low,
                vec!["Podman container query failed with exit 125".to_owned()],
            ),
            (review::Risk::Unknown, vec![]),
        ] {
            let reviewer = Arc::new(FixedReviewer(
                review::ReviewOutcome {
                    risk,
                    authorization: review::Authorization::WithinScope,
                    reason: "Runtime facts need investigation".into(),
                    missing_evidence: facts.clone(),
                },
                std::sync::atomic::AtomicUsize::new(0),
            ));
            let manager = SecurityManager::new(SecurityMode::Auto)
                .with_reviewer(reviewer)
                .with_interaction(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
                    vec![].into(),
                ))));
            manager.set_user_request("Inspect the requested container");
            let args = serde_json::json!({"command":"podman ps -a"});
            let action = ToolAction {
                tool_name: "shell",
                args: &args,
                description: None,
                resolved: None,
            };
            for _ in 0..2 {
                let feedback = manager.authorize(&action).await.unwrap_err();
                assert!(feedback.starts_with("Safety review requires investigation:"));
                assert!(feedback.contains("Runtime facts need investigation"));
                for fact in &facts {
                    assert!(feedback.contains(fact));
                }
                assert!(!feedback.contains("already rejected"));
            }
            let context = manager.context.read();
            assert!(context.denied.is_empty());
            assert!(context
                .history
                .iter()
                .all(|record| record.status == "investigation_required"));
        }
    }

    #[tokio::test]
    async fn high_risk_confirms_once_and_unchanged_rejection_is_cached() {
        let reviewer = fixed(review::Risk::High, review::Authorization::WithinScope);
        let manager = SecurityManager::new(SecurityMode::Auto)
            .with_reviewer(reviewer.clone())
            .with_interaction(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
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
            .with_interaction(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
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
                .with_interaction(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
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
        assert_eq!(manager.context.read().history[0].status, "executed");
        assert_eq!(manager.context.read().history[1].status, "failed");
        manager.set_user_request("second");
        manager.record_execution(one.as_ref(), true);
        assert!(manager.context.read().history.is_empty());
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
                .with_interaction(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
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
        assert!(manager.context.read().history.is_empty());
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
        let manager = SecurityManager::new(SecurityMode::Auto).with_interaction(Arc::new(
            ConfirmSequence(parking_lot::Mutex::new(vec![false].into())),
        ));
        assert!(manager.authorize(&action).await.is_err());
        let manager = SecurityManager::new(SecurityMode::Auto)
            .with_reviewer(fixed(review::Risk::Low, review::Authorization::WithinScope))
            .with_interaction(Arc::new(ConfirmSequence(parking_lot::Mutex::new(
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
        impl HumanInteraction for DenyAll {
            async fn ask(&self, _: &AskRequest) -> AskResult {
                AskResult::Cancelled {
                    reason: AskCancelReason::Unavailable,
                }
            }
            async fn confirm(&self, _: &ConfirmationRequest) -> bool {
                false
            }
        }
        let mgr = SecurityManager::new(SecurityMode::Confirm).with_interaction(Arc::new(DenyAll));
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
