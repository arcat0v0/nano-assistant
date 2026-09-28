pub mod content_search;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob_search;
pub mod pty_shell;
pub mod shell;
pub mod skill_http;
pub mod skill_tool;
pub mod web_fetch;
pub mod web_search;

use rig::tool::server::ToolServerHandle;

pub async fn register_builtin_tools(handle: &ToolServerHandle) {
    handle.add_tool(shell::ShellTool::new()).await;
    handle.add_tool(file_read::FileReadTool::new()).await;
    handle.add_tool(file_write::FileWriteTool::new()).await;
    handle.add_tool(file_edit::FileEditTool::new()).await;
    handle.add_tool(glob_search::GlobSearchTool::new()).await;
    handle
        .add_tool(content_search::ContentSearchTool::new())
        .await;
    handle.add_tool(web_fetch::WebFetchTool::new()).await;
    handle.add_tool(web_search::WebSearchTool::new()).await;
    handle.add_tool(pty_shell::PtyShellTool::new()).await;
}

pub(crate) fn is_protected_skill_path(path: &std::path::Path) -> bool {
    if crate::skills::is_builtin_skill_path(path) {
        return true;
    }
    let mut parent = path.parent();
    while let Some(directory) = parent {
        if let Ok(canonical) = directory.canonicalize() {
            return crate::skills::is_builtin_skill_path(&canonical);
        }
        parent = directory.parent();
    }
    false
}
