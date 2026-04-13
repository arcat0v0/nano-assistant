pub mod content_search;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob_search;
pub mod pty_shell;
pub mod shell;
pub mod skill_http;
pub mod skill_tool;
pub mod traits;
pub mod web_fetch;
pub mod web_search;

pub use traits::{Tool, ToolResult, ToolSpec};

/// Returns the 8 core tools every agent gets by default.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(shell::ShellTool::new()),
        Box::new(file_read::FileReadTool::new()),
        Box::new(file_write::FileWriteTool::new()),
        Box::new(file_edit::FileEditTool::new()),
        Box::new(glob_search::GlobSearchTool::new()),
        Box::new(content_search::ContentSearchTool::new()),
        Box::new(web_fetch::WebFetchTool::new()),
        Box::new(web_search::WebSearchTool::new()),
        Box::new(pty_shell::PtyShellTool::new()),
    ]
}
