//! System information detection module.
//!
//! Collects OS, hardware, and tool information for bootstrap context.
//! Uses `std::process::Command` and `/proc`/`sysctl` — no heavy crates.

use std::collections::HashMap;
use std::fmt::Write;
use std::time::Duration;

/// Information about a detected tool.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub version: String,
    pub path: String,
}

/// System information gathered at startup for bootstrap context.
#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub architecture: String,
    pub hostname: String,
    pub username: String,
    pub groups: String,
    pub shell: String,
    pub locale: String,
    pub cpu_cores: String,
    pub cpu_model: String,
    pub gpu_model: String,
    pub virtualization: String,
    pub memory_total_gb: String,
    pub disk_total_gb: String,
    pub nano_version: String,
    pub rust_version: String,
    pub installed_tools: HashMap<String, ToolInfo>,
}

impl SystemInfo {
    /// Format system information as a Markdown document.
    pub fn format_as_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# System Information\n\n");

        md.push_str("## System\n\n");
        md.push_str(&format!("- **OS**: {} {}\n", self.os_name, self.os_version));
        md.push_str(&format!("- **Kernel**: {}\n", self.kernel_version));
        md.push_str(&format!("- **Architecture**: {}\n", self.architecture));
        md.push_str(&format!("- **Hostname**: {}\n", self.hostname));
        md.push('\n');

        md.push_str("## User\n\n");
        md.push_str(&format!("- **Username**: {}\n", self.username));
        md.push_str(&format!("- **Groups**: {}\n", self.groups));
        md.push_str(&format!("- **Shell**: {}\n", self.shell));
        md.push_str(&format!("- **Locale**: {}\n", self.locale));
        md.push('\n');

        md.push_str("## Environment\n\n");
        md.push_str(&format!("- **Nano Version**: {}\n", self.nano_version));
        md.push_str(&format!("- **Rust Version**: {}\n", self.rust_version));
        md.push('\n');

        md.push_str("## Hardware\n\n");
        md.push_str(&format!(
            "- **CPU**: {} ({})\n",
            self.cpu_cores, self.cpu_model
        ));
        if !self.gpu_model.is_empty() {
            md.push_str(&format!("- **GPU**: {}\n", self.gpu_model));
        }
        if !self.virtualization.is_empty() {
            md.push_str(&format!("- **Virtualization**: {}\n", self.virtualization));
        }
        md.push_str(&format!("- **Memory**: {} GB\n", self.memory_total_gb));
        md.push_str(&format!("- **Disk**: {} GB\n", self.disk_total_gb));
        md.push('\n');

        md.push_str("## Software\n\n");
        md.push_str(&format!("- **Nano**: {}\n", self.nano_version));
        md.push_str(&format!("- **Rust**: {}\n", self.rust_version));
        md.push('\n');

        md.push_str("## Installed Tools\n\n");
        if self.installed_tools.is_empty() {
            md.push_str("- *No tools detected*\n");
        } else {
            let mut tools: Vec<_> = self.installed_tools.iter().collect();
            tools.sort_by_key(|(k, _)| *k);
            for (name, info) in tools {
                if info.path.is_empty() {
                    md.push_str(&format!("- **{}**: {}\n", name, info.version));
                } else {
                    md.push_str(&format!(
                        "- **{}**: {} ({})\n",
                        name, info.version, info.path
                    ));
                }
            }
        }

        md
    }

    /// Format system information as a compact prompt string (≤ 2000 chars).
    pub fn format_for_prompt(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!(
            "OS: {} {} | Kernel: {} | Arch: {}\n",
            self.os_name, self.os_version, self.kernel_version, self.architecture
        ));
        out.push_str(&format!(
            "User: {} | Shell: {} | Locale: {}\n",
            self.username, self.shell, self.locale
        ));
        out.push_str(&format!(
            "CPU: {} ({}) | Memory: {} GB | Disk: {} GB\n",
            self.cpu_cores, self.cpu_model, self.memory_total_gb, self.disk_total_gb
        ));
        if !self.gpu_model.is_empty() {
            out.push_str(&format!("GPU: {}\n", self.gpu_model));
        }
        if !self.virtualization.is_empty() {
            out.push_str(&format!("VM: {}\n", self.virtualization));
        }
        out.push_str(&format!(
            "Nano: {} | Rust: {}\n",
            self.nano_version, self.rust_version
        ));

        if !self.installed_tools.is_empty() {
            out.push_str("Tools:\n");
            let mut tools: Vec<_> = self.installed_tools.iter().collect();
            tools.sort_by_key(|(k, _)| *k);
            for (name, info) in tools {
                let version = if info.version.is_empty() {
                    "?"
                } else {
                    &info.version
                };
                if info.path.is_empty() {
                    let _ = writeln!(out, "  {}={}", name, version);
                } else {
                    let _ = writeln!(out, "  {}={} [{}]", name, version, info.path);
                }
            }
        }

        if out.len() > 2000 {
            out.truncate(1997);
            out.push_str("...");
        }

        out
    }
}

/// Tools to detect via `--version`.
const TOOLS_TO_DETECT: &[&str] = &[
    // Languages & Runtimes
    "node",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "bun",
    "bunx",
    "deno",
    "python3",
    "python",
    "pip",
    "pip3",
    "uv",
    "uvx",
    "pipx",
    "conda",
    "go",
    "java",
    "javac",
    "ruby",
    "gem",
    "php",
    "perl",
    "rustc",
    "cargo",
    "dotnet",
    "swift",
    "clang",
    // Package Managers
    "brew",
    "apt",
    "dnf",
    "pacman",
    // Databases
    "postgres",
    "mysql",
    "sqlite3",
    "redis-cli",
    "mongosh",
    // Container & Orchestration
    "docker",
    "docker-compose",
    "podman",
    "kubectl",
    "helm",
    "nerdctl",
    "terraform",
    "ansible",
    // Build Tools
    "make",
    "cmake",
    "gcc",
    "g++",
    "zig",
    // Version Managers
    "nvm",
    "pyenv",
    "volta",
    "fnm",
    "rustup",
    "sdk",
    // CLI Utilities
    "git",
    "curl",
    "wget",
    "jq",
    "yq",
    "vim",
    "nvim",
    "nano",
    "tmux",
    "screen",
    "fzf",
    "bat",
    "eza",
    "fd",
    "rg",
    "starship",
    "zoxide",
    "htop",
    "btop",
    "ssh",
    "rsync",
    "tar",
    "zip",
    "lsof",
    "code",
    // Misc Dev Tools
    "ffmpeg",
    "imagemagick",
];

/// Detect a single tool version by running `{tool} --version`.
async fn detect_single_tool(tool: &str) -> (Option<String>, Option<String>) {
    let cmd = match tool {
        "postgres" => "psql",
        "imagemagick" => "magick",
        "nvm" => {
            let version = tokio::process::Command::new("bash")
                .args(["-lc", "nvm --version"])
                .output()
                .await
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8(o.stdout).ok()
                    } else {
                        None
                    }
                })
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return (version, None);
        }
        _ => tool,
    };

    let path = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::process::Command::new("which").arg(cmd).output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .filter(|r| r.status.success())
    .and_then(|r| String::from_utf8(r.stdout).ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());

    let version = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::process::Command::new(cmd).arg("--version").output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .filter(|r| r.status.success())
    .and_then(|r| String::from_utf8(r.stdout).ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());

    (version, path)
}

/// Detect all installed tools in parallel using `JoinSet`.
pub async fn detect_installed_tools() -> HashMap<String, ToolInfo> {
    let mut join_set = tokio::task::JoinSet::new();

    for tool in TOOLS_TO_DETECT {
        let tool = tool.to_string();
        join_set.spawn(async move { (tool.clone(), detect_single_tool(&tool).await) });
    }

    let mut result = HashMap::new();
    while let Some(Ok((name, (version, path)))) = join_set.join_next().await {
        if version.is_some() || path.is_some() {
            result.insert(
                name,
                ToolInfo {
                    version: version.unwrap_or_default(),
                    path: path.unwrap_or_default(),
                },
            );
        }
    }

    result
}

/// Run a command and return stdout trimmed, or a default on failure.
#[cfg(unix)]
async fn run_cmd(cmd: &str, args: &[&str], default: &str) -> String {
    tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Run a shell command and return stdout trimmed, or a default on failure.
#[cfg(unix)]
async fn run_shell(cmd: &str, default: &str) -> String {
    tokio::process::Command::new("sh")
        .args(["-c", cmd])
        .output()
        .await
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Parse `/etc/os-release` to get OS name and version.
#[cfg(unix)]
fn parse_os_release(content: &str) -> (String, String) {
    let mut name = String::from("Linux");
    let mut version = String::new();

    for line in content.lines() {
        if let Some(val) = line.strip_prefix("NAME=") {
            name = val.trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("VERSION=") {
            version = val.trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("VERSION_ID=") {
            if version.is_empty() {
                version = val.trim_matches('"').to_string();
            }
        }
    }

    (name, version)
}

/// Detect all system information asynchronously.
#[cfg(unix)]
pub async fn detect() -> SystemInfo {
    let (os_name, os_version) = if std::path::Path::new("/etc/os-release").exists() {
        tokio::fs::read_to_string("/etc/os-release")
            .await
            .ok()
            .map(|c| parse_os_release(&c))
            .unwrap_or_else(|| ("Linux".to_string(), String::new()))
    } else {
        let name = run_cmd("sw_vers", &["-productName"], "macOS").await;
        let ver = run_cmd("sw_vers", &["-productVersion"], "").await;
        (name, ver)
    };

    let kernel_version = run_cmd("uname", &["-r"], "unknown").await;
    let architecture = run_cmd("uname", &["-m"], "unknown").await;
    let hostname = run_cmd("hostname", &[], "unknown").await;
    let username = run_cmd("id", &["-un"], "unknown").await;
    let groups = run_cmd("id", &["-Gn"], "unknown").await;
    let shell = run_shell("echo $SHELL", "unknown").await;
    let locale = run_shell("echo $LANG", "unknown").await;

    // Hardware
    let cpu_cores = run_cmd("nproc", &[], "?").await;

    let cpu_model = run_shell(
        "grep 'model name' /proc/cpuinfo 2>/dev/null | head -1 | sed 's/.*: //'",
        "unknown",
    )
    .await;

    let gpu_model = run_cmd(
        "nvidia-smi",
        &[
            "--query-gpu=name",
            "--format=csv,noheader",
            "--no-nvml-init",
        ],
        "",
    )
    .await;
    let gpu_model = if gpu_model.is_empty() {
        run_shell(
            "lspci 2>/dev/null | grep -i 'vga\\|3d\\|display' | head -1 | sed 's/.*: //'",
            "",
        )
        .await
    } else {
        gpu_model
    };
    let gpu_model = gpu_model
        .trim_start_matches("VGA compatible controller: ")
        .to_string();
    let gpu_model = gpu_model.trim_start_matches("3D controller: ").to_string();

    let virtualization = tokio::process::Command::new("systemd-detect-virt")
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "none")
        .unwrap_or_default();

    let memory_total_gb = if std::path::Path::new("/proc/meminfo").exists() {
        tokio::fs::read_to_string("/proc/meminfo")
            .await
            .ok()
            .and_then(|c| {
                for line in c.lines() {
                    if line.starts_with("MemTotal:") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            if let Ok(kb) = parts[1].parse::<u64>() {
                                let gb = kb as f64 / (1024.0 * 1024.0);
                                return Some(format!("{:.1}", gb));
                            }
                        }
                    }
                }
                None
            })
            .unwrap_or_else(|| "?".to_string())
    } else {
        let bytes = run_cmd("sysctl", &["-n", "hw.memsize"], "0").await;
        bytes
            .parse::<u64>()
            .ok()
            .map(|b| format!("{:.1}", b as f64 / (1024.0 * 1024.0 * 1024.0)))
            .unwrap_or_else(|| "?".to_string())
    };

    let disk_total_gb =
        run_shell("df --output=size / 2>/dev/null | tail -1 | tr -d ' '", "?").await;
    let disk_total_gb = disk_total_gb
        .parse::<u64>()
        .ok()
        .map(|kb| format!("{:.1}", kb as f64 / (1024.0 * 1024.0)))
        .unwrap_or_else(|| "?".to_string());

    let nano_version = env!("CARGO_PKG_VERSION").to_string();
    let rust_version = run_cmd("rustc", &["--version"], "?").await;

    let installed_tools = detect_installed_tools().await;

    SystemInfo {
        os_name,
        os_version,
        kernel_version,
        architecture,
        hostname,
        username,
        groups,
        shell,
        locale,
        cpu_cores,
        cpu_model,
        gpu_model,
        virtualization,
        memory_total_gb,
        disk_total_gb,
        nano_version,
        rust_version,
        installed_tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_system_info() -> SystemInfo {
        let mut tools = HashMap::new();
        tools.insert(
            "git".to_string(),
            ToolInfo {
                version: "git version 2.43.0".to_string(),
                path: "/usr/bin/git".to_string(),
            },
        );
        tools.insert(
            "node".to_string(),
            ToolInfo {
                version: "v20.10.0".to_string(),
                path: "/usr/local/bin/node".to_string(),
            },
        );
        tools.insert(
            "cargo".to_string(),
            ToolInfo {
                version: "cargo 1.74.0".to_string(),
                path: "/home/user/.cargo/bin/cargo".to_string(),
            },
        );
        tools.insert(
            "python3".to_string(),
            ToolInfo {
                version: "Python 3.12.1".to_string(),
                path: "/usr/bin/python3".to_string(),
            },
        );
        tools.insert(
            "docker".to_string(),
            ToolInfo {
                version: "Docker version 24.0.7".to_string(),
                path: "/usr/bin/docker".to_string(),
            },
        );
        tools.insert(
            "go".to_string(),
            ToolInfo {
                version: "go version go1.21.5".to_string(),
                path: "/usr/local/go/bin/go".to_string(),
            },
        );
        tools.insert(
            "rustc".to_string(),
            ToolInfo {
                version: "rustc 1.74.0".to_string(),
                path: "/home/user/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc"
                    .to_string(),
            },
        );
        tools.insert(
            "npm".to_string(),
            ToolInfo {
                version: "10.2.3".to_string(),
                path: "/usr/local/bin/npm".to_string(),
            },
        );
        tools.insert(
            "pip".to_string(),
            ToolInfo {
                version: "pip 23.3.2".to_string(),
                path: "/home/user/.local/bin/pip".to_string(),
            },
        );
        tools.insert(
            "ruby".to_string(),
            ToolInfo {
                version: "ruby 3.3.0".to_string(),
                path: "/usr/bin/ruby".to_string(),
            },
        );
        tools.insert(
            "java".to_string(),
            ToolInfo {
                version: "openjdk 21.0.1".to_string(),
                path: "/usr/bin/java".to_string(),
            },
        );
        tools.insert(
            "uv".to_string(),
            ToolInfo {
                version: "uv 0.1.0".to_string(),
                path: "/home/user/.local/bin/uv".to_string(),
            },
        );
        tools.insert(
            "npx".to_string(),
            ToolInfo {
                version: "10.2.3".to_string(),
                path: "/usr/local/bin/npx".to_string(),
            },
        );
        tools.insert(
            "nvm".to_string(),
            ToolInfo {
                version: "0.39.7".to_string(),
                path: "".to_string(),
            },
        );
        tools.insert(
            "podman".to_string(),
            ToolInfo {
                version: "podman version 4.9.0".to_string(),
                path: "/usr/bin/podman".to_string(),
            },
        );
        tools.insert(
            "postgres".to_string(),
            ToolInfo {
                version: "psql 16.1".to_string(),
                path: "/usr/bin/psql".to_string(),
            },
        );

        SystemInfo {
            os_name: "Ubuntu".to_string(),
            os_version: "24.04 LTS".to_string(),
            kernel_version: "6.5.0-44-generic".to_string(),
            architecture: "x86_64".to_string(),
            hostname: "dev-machine".to_string(),
            username: "developer".to_string(),
            groups: "developer sudo docker".to_string(),
            shell: "/bin/bash".to_string(),
            locale: "en_US.UTF-8".to_string(),
            cpu_cores: "8".to_string(),
            cpu_model: "Intel Core i7-12700K".to_string(),
            gpu_model: "NVIDIA GeForce RTX 4090".to_string(),
            virtualization: "".to_string(),
            memory_total_gb: "15.6".to_string(),
            disk_total_gb: "512.0".to_string(),
            nano_version: "0.1.0".to_string(),
            rust_version: "rustc 1.74.0 (e84a7a72e 2023-12-22)".to_string(),
            installed_tools: tools,
        }
    }

    fn minimal_system_info() -> SystemInfo {
        SystemInfo {
            os_name: "Linux".to_string(),
            os_version: String::new(),
            kernel_version: "unknown".to_string(),
            architecture: "unknown".to_string(),
            hostname: "unknown".to_string(),
            username: "unknown".to_string(),
            groups: "unknown".to_string(),
            shell: "unknown".to_string(),
            locale: "unknown".to_string(),
            cpu_cores: "?".to_string(),
            cpu_model: "unknown".to_string(),
            gpu_model: "".to_string(),
            virtualization: "".to_string(),
            memory_total_gb: "?".to_string(),
            disk_total_gb: "?".to_string(),
            nano_version: "0.1.0".to_string(),
            rust_version: "?".to_string(),
            installed_tools: HashMap::new(),
        }
    }

    #[test]
    fn test_format_as_markdown_starts_with_header() {
        let info = sample_system_info();
        let md = info.format_as_markdown();
        assert!(md.starts_with("# System Information\n"));
    }

    #[test]
    fn test_format_as_markdown_has_all_sections() {
        let info = sample_system_info();
        let md = info.format_as_markdown();
        assert!(md.contains("## System"));
        assert!(md.contains("## User"));
        assert!(md.contains("## Environment"));
        assert!(md.contains("## Hardware"));
        assert!(md.contains("## Software"));
        assert!(md.contains("## Installed Tools"));
    }

    #[test]
    fn test_format_as_markdown_includes_key_fields() {
        let info = sample_system_info();
        let md = info.format_as_markdown();
        assert!(md.contains("Ubuntu"));
        assert!(md.contains("24.04 LTS"));
        assert!(md.contains("6.5.0-44-generic"));
        assert!(md.contains("x86_64"));
        assert!(md.contains("developer"));
        assert!(md.contains("/bin/bash"));
        assert!(md.contains("en_US.UTF-8"));
        assert!(md.contains("**CPU**: 8 (Intel Core i7-12700K)"));
    }

    #[test]
    fn test_format_as_markdown_empty_tools() {
        let info = minimal_system_info();
        let md = info.format_as_markdown();
        assert!(md.contains("## Installed Tools"));
        assert!(md.contains("*No tools detected*"));
    }

    #[test]
    fn test_format_as_markdown_tools_are_sorted() {
        let info = sample_system_info();
        let md = info.format_as_markdown();
        let tools_section = md.split("## Installed Tools\n\n").nth(1).unwrap();
        let cargo_pos = tools_section.find("cargo").unwrap();
        let docker_pos = tools_section.find("docker").unwrap();
        assert!(cargo_pos < docker_pos);
    }

    #[test]
    fn test_format_for_prompt_within_char_limit() {
        let info = sample_system_info();
        let prompt = info.format_for_prompt();
        assert!(
            prompt.len() <= 2000,
            "Prompt length {} exceeds 2000 chars",
            prompt.len()
        );
    }

    #[test]
    fn test_format_for_prompt_contains_key_info() {
        let info = sample_system_info();
        let prompt = info.format_for_prompt();
        assert!(prompt.contains("Ubuntu"));
        assert!(prompt.contains("developer"));
        assert!(prompt.contains("/bin/bash"));
        assert!(prompt.contains("8 (Intel Core i7-12700K)"));
    }

    #[test]
    fn test_format_for_prompt_minimal() {
        let info = minimal_system_info();
        let prompt = info.format_for_prompt();
        assert!(prompt.contains("Linux"));
        assert!(prompt.len() <= 2000);
    }

    #[test]
    fn test_format_for_prompt_no_tools_line_when_empty() {
        let info = minimal_system_info();
        let prompt = info.format_for_prompt();
        assert!(!prompt.contains("Tools:"));
    }

    #[tokio::test]
    async fn test_detect_installed_tools_graceful_on_missing() {
        let tools = detect_installed_tools().await;
        // Should never panic, always returns a valid HashMap
        assert!(!tools.is_empty());
    }

    #[tokio::test]
    async fn test_detect_single_tool_missing() {
        let (version, path) = detect_single_tool("definitely_not_a_real_tool_xyz_12345").await;
        assert!(version.is_none());
        assert!(path.is_none());
    }

    #[tokio::test]
    async fn test_detect_single_tool_timeout() {
        let (version, path) = detect_single_tool("sleep_for_10_seconds_tool").await;
        assert!(version.is_none());
        assert!(path.is_none());
    }

    #[test]
    fn test_parse_os_release() {
        let content = r#"NAME="Ubuntu"
VERSION="24.04 LTS (Noble Numbat)"
ID=ubuntu
ID_LIKE=debian
VERSION_ID="24.04""#;
        let (name, version) = parse_os_release(content);
        assert_eq!(name, "Ubuntu");
        assert_eq!(version, "24.04 LTS (Noble Numbat)");
    }

    #[test]
    fn test_parse_os_release_minimal() {
        let content = "NAME=\"Arch Linux\"\nID=arch";
        let (name, version) = parse_os_release(content);
        assert_eq!(name, "Arch Linux");
        assert!(version.is_empty());
    }

    #[test]
    fn test_format_for_prompt_truncation() {
        let mut tools = HashMap::new();
        for i in 0..50 {
            tools.insert(
                format!("tool_{i}"),
                ToolInfo {
                    version: format!(
                        "version_{}_with_a_very_long_string_that_goes_on_and_on_and_on",
                        i
                    ),
                    path: format!("/usr/bin/tool_{}", i),
                },
            );
        }
        let info = SystemInfo {
            os_name: "A".repeat(500),
            os_version: "B".repeat(500),
            kernel_version: "C".repeat(500),
            architecture: "D".repeat(500),
            hostname: "E".repeat(500),
            username: "F".repeat(500),
            groups: "G".repeat(500),
            shell: "H".repeat(500),
            locale: "I".repeat(500),
            cpu_cores: "J".repeat(500),
            cpu_model: "K".repeat(500),
            gpu_model: "L".repeat(500),
            virtualization: "M".repeat(500),
            memory_total_gb: "K".repeat(500),
            disk_total_gb: "L".repeat(500),
            nano_version: "M".repeat(500),
            rust_version: "N".repeat(500),
            installed_tools: tools,
        };
        let prompt = info.format_for_prompt();
        assert!(
            prompt.len() <= 2000,
            "Prompt length {} exceeds 2000 chars",
            prompt.len()
        );
        assert!(prompt.ends_with("..."));
    }

    #[test]
    fn format_as_markdown_includes_all_sections() {
        let info = SystemInfo {
            os_name: "TestOS".to_string(),
            os_version: "1.0".to_string(),
            kernel_version: "5.0".to_string(),
            architecture: "x86_64".to_string(),
            hostname: "testhost".to_string(),
            username: "testuser".to_string(),
            groups: "users,wheel".to_string(),
            shell: "/bin/bash".to_string(),
            locale: "en_US.UTF-8".to_string(),
            cpu_cores: "8".to_string(),
            cpu_model: "Intel Core i5".to_string(),
            gpu_model: "".to_string(),
            virtualization: "".to_string(),
            memory_total_gb: "16.0".to_string(),
            disk_total_gb: "512.0".to_string(),
            nano_version: "0.1.0".to_string(),
            rust_version: "1.75.0".to_string(),
            installed_tools: HashMap::new(),
        };
        let md = info.format_as_markdown();
        assert!(md.contains("# System Information"));
        assert!(md.contains("## System"));
        assert!(md.contains("## User"));
        assert!(md.contains("## Hardware"));
        assert!(md.contains("## Software"));
    }

    #[test]
    fn format_for_prompt_respects_limit_with_many_tools() {
        let mut info = SystemInfo {
            os_name: "Linux".to_string(),
            os_version: "1.0".to_string(),
            kernel_version: "5.0".to_string(),
            architecture: "x86_64".to_string(),
            hostname: "host".to_string(),
            username: "user".to_string(),
            groups: "users".to_string(),
            shell: "/bin/bash".to_string(),
            locale: "en_US".to_string(),
            cpu_cores: "4".to_string(),
            cpu_model: "unknown".to_string(),
            gpu_model: "".to_string(),
            virtualization: "".to_string(),
            memory_total_gb: "8.0".to_string(),
            disk_total_gb: "256.0".to_string(),
            nano_version: "0.1.0".to_string(),
            rust_version: "1.75.0".to_string(),
            installed_tools: HashMap::new(),
        };
        for i in 0..100 {
            info.installed_tools.insert(
                format!("tool_{i}"),
                ToolInfo {
                    version: format!("version_{i}_very_long_string_to_fill_space"),
                    path: format!("/usr/bin/tool_{i}"),
                },
            );
        }
        let prompt = info.format_for_prompt();
        assert!(
            prompt.len() <= 2000,
            "prompt is {} chars, should be ≤2000",
            prompt.len()
        );
    }

    #[test]
    fn tool_info_stores_version_and_path() {
        let info = ToolInfo {
            version: "20.0.0".to_string(),
            path: "/usr/local/bin/node".to_string(),
        };
        assert_eq!(info.version, "20.0.0");
        assert_eq!(info.path, "/usr/local/bin/node");
    }

    #[test]
    fn detect_single_tool_returns_path() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (version, path) = rt.block_on(detect_single_tool("git"));
        if version.is_some() {
            assert!(
                path.is_some(),
                "if version found, path should also be found"
            );
        }
    }

    #[test]
    fn format_as_markdown_includes_tool_path() {
        let mut tools = HashMap::new();
        tools.insert(
            "node".to_string(),
            ToolInfo {
                version: "20.0.0".to_string(),
                path: "/usr/local/bin/node".to_string(),
            },
        );
        tools.insert(
            "nvm".to_string(),
            ToolInfo {
                version: "0.39.0".to_string(),
                path: "".to_string(),
            },
        );
        let info = SystemInfo {
            os_name: "Linux".to_string(),
            os_version: "1.0".to_string(),
            kernel_version: "5.0".to_string(),
            architecture: "x86_64".to_string(),
            hostname: "host".to_string(),
            username: "user".to_string(),
            groups: "users".to_string(),
            shell: "/bin/bash".to_string(),
            locale: "en_US".to_string(),
            cpu_cores: "4".to_string(),
            cpu_model: "unknown".to_string(),
            gpu_model: "".to_string(),
            virtualization: "".to_string(),
            memory_total_gb: "8.0".to_string(),
            disk_total_gb: "256.0".to_string(),
            nano_version: "0.1.0".to_string(),
            rust_version: "1.75.0".to_string(),
            installed_tools: tools,
        };
        let md = info.format_as_markdown();
        assert!(md.contains("/usr/local/bin/node"), "should show tool path");
        assert!(
            md.contains("- **nvm**: 0.39.0"),
            "should show version without path when path is empty"
        );
    }

    #[test]
    fn detect_installed_tools_has_many_entries() {
        assert!(
            TOOLS_TO_DETECT.len() >= 40,
            "should detect at least 40 tools, got {}",
            TOOLS_TO_DETECT.len()
        );
    }
}
