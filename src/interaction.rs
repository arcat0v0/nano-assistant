use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRequest {
    pub questions: Vec<AskQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskQuestion {
    pub id: String,
    pub header: Option<String>,
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskOption>,
    #[serde(default)]
    pub multi: bool,
    pub recommended: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AskAnswer {
    pub question_id: String,
    pub selected: Vec<String>,
    pub custom: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AskResult {
    Answered { answers: Vec<AskAnswer> },
    Cancelled { reason: AskCancelReason },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskCancelReason {
    Cancelled,
    Interrupted,
    Eof,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct ConfirmationRequest {
    pub tool_name: String,
    pub summary: String,
    pub details: String,
    pub reason: Option<String>,
    pub missing_evidence: Vec<String>,
    pub risk_label: Option<String>,
    pub preview: Option<String>,
}

#[async_trait]
pub trait HumanInteraction: Send + Sync {
    async fn ask(&self, request: &AskRequest) -> AskResult;
    async fn confirm(&self, request: &ConfirmationRequest) -> bool;
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != "__custom"
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn text(value: &str, max: usize, required: bool) -> Result<(), String> {
    if value.chars().count() > max || (required && value.trim().is_empty()) {
        Err(format!(
            "Text must be {} and at most {max} Unicode characters",
            if required { "nonempty" } else { "valid" }
        ))
    } else {
        Ok(())
    }
}

impl AskRequest {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=4).contains(&self.questions.len()) {
            return Err("Ask requires 1–4 questions".into());
        }
        let mut ids = HashSet::new();
        for q in &self.questions {
            if !valid_id(&q.id) || !ids.insert(&q.id) {
                return Err("Invalid or duplicate question id".into());
            }
            text(&q.question, 1024, true)?;
            if let Some(h) = &q.header {
                text(h, 24, false)?;
            }
            if q.options.is_empty() {
                if q.multi || q.recommended.is_some() {
                    return Err("Text questions cannot use multi or recommended".into());
                }
            } else if !(2..=6).contains(&q.options.len()) {
                return Err("Options require 2–6 entries".into());
            }
            let mut opts = HashSet::new();
            for o in &q.options {
                if !valid_id(&o.id) || !opts.insert(&o.id) {
                    return Err("Invalid or duplicate option id".into());
                }
                text(&o.label, 96, true)?;
                if let Some(d) = &o.description {
                    text(d, 256, false)?;
                }
            }
            if q.recommended.as_ref().is_some_and(|r| !opts.contains(r)) {
                return Err("Recommended must reference an option".into());
            }
        }
        Ok(())
    }

    pub fn validate_answers(&self, answers: &[AskAnswer]) -> Result<(), String> {
        self.validate()?;
        if answers.len() != self.questions.len() {
            return Err("Every question requires an answer".into());
        }
        let mut seen = HashSet::new();
        for a in answers {
            let q = self
                .questions
                .iter()
                .find(|q| q.id == a.question_id)
                .ok_or("Unknown question id")?;
            if !seen.insert(&a.question_id) {
                return Err("Duplicate answer".into());
            }
            let mut selected = HashSet::new();
            for id in &a.selected {
                if !q.options.iter().any(|o| &o.id == id) || !selected.insert(id) {
                    return Err("Invalid or duplicate selected option".into());
                }
            }
            if let Some(c) = &a.custom {
                text(c, 2048, true)?;
                if c.contains('\0') {
                    return Err("Custom answer contains NUL".into());
                }
            }
            let count = a.selected.len() + usize::from(a.custom.is_some());
            if count == 0 || (!q.multi && count != 1) {
                return Err("Invalid selection count".into());
            }
        }
        Ok(())
    }

    pub fn schema() -> Value {
        let id = json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^(?!__custom$)[A-Za-z0-9_-]+$"});
        let option = json!({"type":"object","additionalProperties":false,"required":["id","label"],"properties":{
            "id":id,"label":{"type":"string","minLength":1,"maxLength":96,"pattern":"\\S"},
            "description":{"type":"string","maxLength":256}}});
        json!({"type":"object","additionalProperties":false,"required":["questions"],"properties":{"questions":{
            "type":"array","minItems":1,"maxItems":4,"uniqueItems":true,"description":"Question ids must be unique within the batch.","items":{"type":"object","additionalProperties":false,
            "required":["id","question"],"properties":{"id":id,"header":{"type":"string","maxLength":24},
            "question":{"type":"string","minLength":1,"maxLength":1024,"pattern":"\\S"},
            "options":{"type":"array","items":option,"maxItems":6,"uniqueItems":true,"description":"Option ids must be unique within this question.","anyOf":[{"maxItems":0},{"minItems":2}]},
            "multi":{"type":"boolean","default":false},"recommended":{"type":"string","minLength":1,"maxLength":64,"pattern":"^(?!__custom$)[A-Za-z0-9_-]+$","description":"Must be the id of an option in this question; focus only, never an automatic answer."}},
            "allOf":[{"if":{"anyOf":[{"not":{"required":["options"]}},{"properties":{"options":{"maxItems":0}},"required":["options"]}]},
            "then":{"properties":{"multi":{"const":false}},"not":{"required":["recommended"]}}}]}}}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> AskRequest {
        serde_json::from_value(serde_json::json!({"questions":[{"id":"policy","question":"数据？","options":[{"id":"keep","label":"保留"},{"id":"remove","label":"删除"}],"recommended":"keep"}]})).unwrap()
    }
    #[test]
    fn interaction_request_validation_and_strict_fields() {
        let mut r = request();
        assert!(r.validate().is_ok());
        r.questions[0].options[1].id = "keep".into();
        assert!(r.validate().is_err());
        assert!(serde_json::from_value::<AskRequest>(
            serde_json::json!({"questions":[],"extra":true})
        )
        .is_err());
        r = request();
        r.questions[0].id = "__custom".into();
        assert!(r.validate().is_err());
        r = request();
        r.questions[0].question = "中".repeat(1025);
        assert!(r.validate().is_err());
    }
    #[test]
    fn interaction_answers_are_bound_to_questions() {
        let r = request();
        let mut a = vec![AskAnswer {
            question_id: "policy".into(),
            selected: vec!["keep".into()],
            custom: None,
        }];
        assert!(r.validate_answers(&a).is_ok());
        a[0].selected.push("remove".into());
        assert!(r.validate_answers(&a).is_err());
        a[0].selected.clear();
        a[0].custom = Some("中文路径".into());
        assert!(r.validate_answers(&a).is_ok());
        a[0].custom = Some(" ".into());
        assert!(r.validate_answers(&a).is_err());
    }
    #[test]
    fn interaction_cancel_result_serialization() {
        assert_eq!(
            serde_json::to_value(AskResult::Cancelled {
                reason: AskCancelReason::Eof
            })
            .unwrap(),
            serde_json::json!({"status":"cancelled","reason":"eof"})
        );
    }
}
