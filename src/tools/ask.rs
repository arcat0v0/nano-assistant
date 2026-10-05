use crate::interaction::AskRequest;
use crate::security::SecurityManager;
use rig::tool::{Tool, ToolContext, ToolExecutionError};
use serde_json::Value;
use std::sync::Arc;

pub struct AskTool {
    security: Arc<SecurityManager>,
}

impl AskTool {
    pub fn new(security: Arc<SecurityManager>) -> Self {
        Self { security }
    }
}

impl Tool for AskTool {
    const NAME: &'static str = "ask";
    type Args = Value;
    type Output = crate::interaction::AskResult;
    type Error = ToolExecutionError;

    fn description(&self) -> String {
        "Ask the real user for non-sensitive task details that cannot be established from context or inspection. Batch related questions. Never request passwords or API keys; answers clarify scope, not safety approval.".into()
    }

    fn parameters(&self) -> Value {
        AskRequest::schema()
    }

    async fn call(&self, _: &mut ToolContext, args: Value) -> Result<Self::Output, Self::Error> {
        let request: AskRequest = serde_json::from_value(args)
            .map_err(|error| ToolExecutionError::invalid_args(error.to_string()))?;
        self.security.ask(&request).await
    }
}
