use crate::security::SecurityDecision;
use crate::tools::traits::{Tool, ToolResult};

pub fn check_whitelist(command: &str, whitelist: &[String]) -> SecurityDecision {
    if whitelist.is_empty() {
        return SecurityDecision::Deny("whitelist is empty — no commands allowed".into());
    }

    let matched = whitelist
        .iter()
        .any(|pattern| match_command(pattern, command));

    if matched {
        SecurityDecision::Allow
    } else {
        SecurityDecision::Deny(format!("command not in whitelist: {command}"))
    }
}

pub async fn execute(
    tool: &dyn Tool,
    args: serde_json::Value,
    whitelist: &[String],
) -> anyhow::Result<ToolResult> {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;

    match check_whitelist(command, whitelist) {
        SecurityDecision::Allow => tool.execute(args).await,
        SecurityDecision::Deny(reason) => Ok(ToolResult {
            success: false,
            output: String::new(),
            error: Some(reason),
        }),
    }
}

fn match_command(pattern: &str, command: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split_whitespace().collect();
    let command_parts: Vec<&str> = command.split_whitespace().collect();

    if pattern_parts.is_empty() {
        return false;
    }

    match_glob_parts(&pattern_parts, &command_parts)
}

fn match_glob_parts(pattern: &[&str], command: &[&str]) -> bool {
    if pattern.is_empty() && command.is_empty() {
        return true;
    }
    if pattern.is_empty() {
        return false;
    }

    if pattern[0] == "*" {
        for i in 0..=command.len() {
            if match_glob_parts(&pattern[1..], &command[i..]) {
                return true;
            }
        }
        return false;
    }

    if command.is_empty() {
        return false;
    }

    if glob_match(pattern[0], command[0]) {
        match_glob_parts(&pattern[1..], &command[1..])
    } else {
        false
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    match glob::Pattern::new(pattern) {
        Ok(p) => p.matches(text),
        Err(_) => pattern == text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(match_command("ls", "ls"));
        assert!(!match_command("ls", "cat"));
    }

    #[test]
    fn wildcard_at_end() {
        assert!(match_command("docker *", "docker ps"));
        assert!(match_command("docker *", "docker run -it ubuntu"));
        assert!(match_command("docker *", "docker"));
    }

    #[test]
    fn wildcard_in_middle() {
        assert!(match_command("systemctl * nginx", "systemctl status nginx"));
        assert!(match_command(
            "systemctl * nginx",
            "systemctl restart nginx"
        ));
        assert!(!match_command(
            "systemctl * nginx",
            "systemctl status apache2"
        ));
    }

    #[test]
    fn multiple_wildcards() {
        assert!(match_command("* *", "any thing here"));
        assert!(match_command("* *", "a b"));
    }

    #[test]
    fn glob_token_matching() {
        assert!(match_command(
            "systemctl status *",
            "systemctl status nginx"
        ));
        assert!(match_command(
            "systemctl status *",
            "systemctl status mysql"
        ));
        assert!(match_command("cat /etc/*", "cat /etc/hosts"));
        assert!(match_command("cat /etc/*", "cat /etc/passwd"));
        assert!(!match_command("cat /etc/*", "cat /var/log/syslog"));
    }

    #[test]
    fn empty_pattern() {
        assert!(!match_command("", "ls"));
    }

    #[test]
    fn whitelist_check_allows_matching() {
        let wl = vec!["ls".into(), "docker *".into()];
        assert_eq!(check_whitelist("ls", &wl), SecurityDecision::Allow);
        assert_eq!(check_whitelist("docker ps", &wl), SecurityDecision::Allow);
        assert_eq!(
            check_whitelist("docker run hello", &wl),
            SecurityDecision::Allow
        );
    }

    #[test]
    fn whitelist_check_denies_non_matching() {
        let wl = vec!["ls".into()];
        assert!(matches!(
            check_whitelist("rm", &wl),
            SecurityDecision::Deny(_)
        ));
    }

    #[test]
    fn empty_whitelist_denies_all() {
        assert!(matches!(
            check_whitelist("ls", &[]),
            SecurityDecision::Deny(msg) if msg.contains("empty")
        ));
    }
}
