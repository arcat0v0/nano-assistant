//! Markdown-based memory backend.
//!
//! Stores memories as human-readable Markdown files. A single file (`memory.md`)
//! acts as the source of truth with entries formatted as:
//!
//! ```markdown
//! ## 2024-01-15 14:30:00 - conversation
//! - **Key**: nginx-install
//! - **Content**: 用户询问如何安装 nginx
//! - **Category**: conversation
//! - **Tags**: nginx, install
//! - **Session**: s1
//! ```

use super::traits::{Memory, MemoryCategory, MemoryEntry};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

const DOCUMENT_HEADER: &str = "# Nano-Assistant Memory\n\n";
const SYSTEM_INFO_START: &str = "<!-- SYSTEM_INFO_START -->";
const SYSTEM_INFO_END: &str = "<!-- SYSTEM_INFO_END -->";

/// Markdown-based memory backend.
///
/// Uses a single Markdown file as persistent storage. Entries are
/// appended as sections and parsed back on query/delete operations.
pub struct MarkdownMemory {
    path: PathBuf,
}

impl MarkdownMemory {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn strip_document_header(content: &str) -> &str {
        let trimmed = content.trim();
        if let Some(rest) = trimmed.strip_prefix("# Nano-Assistant Memory") {
            rest.trim_start_matches('\n').trim_start()
        } else {
            trimmed
        }
    }

    fn demote_system_info_headings(content: &str) -> String {
        let mut out = String::new();
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                out.push_str("### ");
                out.push_str(rest);
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        out.trim().to_string()
    }

    pub fn normalize_system_info_markdown(markdown: &str) -> String {
        let trimmed = markdown.trim();
        let body = if let Some(rest) = trimmed.strip_prefix("# System Information") {
            rest.trim_start_matches('\n').trim_start()
        } else if let Some(rest) = trimmed.strip_prefix("## System Information") {
            rest.trim_start_matches('\n').trim_start()
        } else {
            trimmed
        };

        Self::demote_system_info_headings(body)
    }

    fn system_info_block_range(content: &str) -> Option<(usize, usize)> {
        let start = content.find(SYSTEM_INFO_START)?;
        let after_start = start + SYSTEM_INFO_START.len();
        let end_rel = content[after_start..].find(SYSTEM_INFO_END)?;
        let end = after_start + end_rel + SYSTEM_INFO_END.len();
        Some((start, end))
    }

    pub fn extract_system_info_markdown(content: &str) -> Option<String> {
        if let Some((start, end)) = Self::system_info_block_range(content) {
            let after_start = start + SYSTEM_INFO_START.len();
            let before_end = end - SYSTEM_INFO_END.len();
            let block = content[after_start..before_end].trim();
            return Some(Self::normalize_system_info_markdown(block));
        }

        let trimmed = content.trim();
        if trimmed.starts_with("# System Information")
            || trimmed.starts_with("## System Information")
        {
            return Some(Self::normalize_system_info_markdown(trimmed));
        }

        None
    }

    fn entries_to_markdown<'a, I>(entries: I) -> String
    where
        I: IntoIterator<Item = &'a MemoryEntry>,
    {
        let mut out = String::new();
        for entry in entries {
            out.push_str(&Self::format_entry(entry));
            out.push('\n');
        }
        out.trim().to_string()
    }

    fn compose_document(system_info: Option<&str>, tail: Option<&str>) -> String {
        let mut out = String::from(DOCUMENT_HEADER);

        if let Some(system_info) = system_info.filter(|s| !s.trim().is_empty()) {
            out.push_str(SYSTEM_INFO_START);
            out.push('\n');
            out.push_str("## System Information\n\n");
            out.push_str(system_info.trim());
            out.push('\n');
            out.push_str(SYSTEM_INFO_END);
        }

        if let Some(tail) = tail.filter(|s| !s.trim().is_empty()) {
            if !out.ends_with("\n\n") {
                out.push_str("\n\n");
            }
            out.push_str(tail.trim());
            out.push('\n');
        }

        out
    }

    pub fn upsert_system_info_markdown(content: &str, system_info_markdown: &str) -> String {
        let normalized = Self::normalize_system_info_markdown(system_info_markdown);

        if let Some((_, end)) = Self::system_info_block_range(content) {
            let tail = content[end..].trim();
            return Self::compose_document(Some(&normalized), Some(tail));
        }

        let trimmed = content.trim();
        if trimmed.starts_with("# System Information")
            || trimmed.starts_with("## System Information")
        {
            let entries = Self::parse_entries(trimmed);
            let tail = Self::entries_to_markdown(entries.iter());
            return Self::compose_document(Some(&normalized), Some(&tail));
        }

        let tail = Self::strip_document_header(trimmed);
        Self::compose_document(Some(&normalized), Some(tail))
    }

    fn now_timestamp() -> String {
        chrono_now()
    }

    fn generate_id() -> String {
        uuid::Uuid::new_v4().to_string()[..8].to_string()
    }

    fn format_entry(entry: &MemoryEntry) -> String {
        format!(
            "## {} - {}\n\
             - **Key**: {}\n\
             - **Content**: {}\n\
             - **Category**: {}\n\
             - **Session**: {}\n",
            entry.timestamp,
            entry.category,
            entry.key,
            entry.content,
            entry.category,
            entry.session_id.as_deref().unwrap_or("none"),
        )
    }

    fn parse_entries(content: &str) -> Vec<MemoryEntry> {
        let mut entries = Vec::new();
        let mut current_key = String::new();
        let mut current_content = String::new();
        let mut current_category = MemoryCategory::Conversation;
        let mut current_timestamp = String::new();
        let mut current_session_id: Option<String> = None;
        let mut current_id = String::new();
        let mut in_section = false;

        for line in content.lines() {
            if line.starts_with("## ") {
                if in_section && !current_key.is_empty() {
                    entries.push(MemoryEntry {
                        id: current_id.clone(),
                        key: current_key.clone(),
                        content: current_content.clone(),
                        category: current_category.clone(),
                        timestamp: current_timestamp.clone(),
                        session_id: current_session_id.clone(),
                        score: None,
                    });
                }
                in_section = true;
                current_key.clear();
                current_content.clear();
                current_session_id = None;
                current_id = Self::generate_id();

                let header = line.trim_start_matches("## ");
                if let Some(cat_pos) = header.rfind(" - ") {
                    let ts = &header[..cat_pos];
                    let cat_str = &header[cat_pos + 3..];
                    current_timestamp = ts.to_string();
                    current_category = parse_category(cat_str);
                } else {
                    current_timestamp = header.to_string();
                }
            } else if let Some(rest) = line.trim().strip_prefix("- ") {
                if let Some(colon_pos) = rest.find(": ") {
                    let field = &rest[..colon_pos];
                    let value = &rest[colon_pos + 2..];
                    match field {
                        "**Key**" => current_key = value.to_string(),
                        "**Content**" => current_content = value.to_string(),
                        "**Category**" => current_category = parse_category(value),
                        "**Session**" if value != "none" => {
                            current_session_id = Some(value.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }

        if in_section && !current_key.is_empty() {
            entries.push(MemoryEntry {
                id: current_id,
                key: current_key,
                content: current_content,
                category: current_category,
                timestamp: current_timestamp,
                session_id: current_session_id,
                score: None,
            });
        }

        entries
    }

    async fn read_file(&self) -> anyhow::Result<String> {
        if !self.path.exists() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(&self.path).await?)
    }

    async fn write_file(&self, content: &str) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await?;
        }
        Ok(fs::write(&self.path, content).await?)
    }

    async fn ensure_header(&self) -> anyhow::Result<()> {
        if !self.path.exists() {
            self.write_file(DOCUMENT_HEADER).await?;
        }
        Ok(())
    }
}

fn parse_category(s: &str) -> MemoryCategory {
    match s.trim() {
        "core" => MemoryCategory::Core,
        "daily" => MemoryCategory::Daily,
        "conversation" => MemoryCategory::Conversation,
        other => MemoryCategory::Custom(other.to_string()),
    }
}

/// Generate an ISO-8601-ish timestamp without depending on `chrono`.
fn chrono_now() -> String {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;

    // Simplified date calculation from epoch
    let mut year = 1970i32;
    let mut remaining_days = days;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days: [u32; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 0usize;
    for &md in &month_days {
        if remaining_days < md as u64 {
            break;
        }
        remaining_days -= md as u64;
        month += 1;
    }
    let day = remaining_days + 1;

    let secs_in_day = secs % 86400;
    let hour = secs_in_day / 3600;
    let minute = (secs_in_day % 3600) / 60;
    let second = secs_in_day % 60;

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year,
        month + 1,
        day,
        hour,
        minute,
        second,
    )
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[async_trait]
impl Memory for MarkdownMemory {
    fn name(&self) -> &str {
        "markdown"
    }

    async fn add(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let timestamp = Self::now_timestamp();
        let entry = MemoryEntry {
            id: Self::generate_id(),
            key: key.to_string(),
            content: content.to_string(),
            category: category.clone(),
            timestamp,
            session_id: session_id.map(|s| s.to_string()),
            score: None,
        };

        self.ensure_header().await?;
        let mut file_content = self.read_file().await?;
        if file_content.trim().is_empty() {
            file_content = DOCUMENT_HEADER.to_string();
        }
        if !file_content.ends_with("\n\n") {
            if file_content.ends_with('\n') {
                file_content.push('\n');
            } else {
                file_content.push_str("\n\n");
            }
        }
        file_content.push_str(&Self::format_entry(&entry));
        file_content.push('\n');
        self.write_file(&file_content).await
    }

    async fn query(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let content = self.read_file().await?;
        let mut entries = Self::parse_entries(&content);

        if let Some(sid) = session_id {
            entries.retain(|e| e.session_id.as_deref().is_some_and(|s| s == sid));
        }

        let query_lower = query.to_lowercase();
        let keywords: Vec<&str> = query_lower.split_whitespace().collect();

        if !keywords.is_empty() {
            entries.retain(|e| {
                let content_lower = e.content.to_lowercase();
                let key_lower = e.key.to_lowercase();
                keywords
                    .iter()
                    .any(|kw| content_lower.contains(kw) || key_lower.contains(kw))
            });

            for entry in &mut entries {
                let content_lower = entry.content.to_lowercase();
                let key_lower = entry.key.to_lowercase();
                let matched = keywords
                    .iter()
                    .filter(|kw| content_lower.contains(*kw) || key_lower.contains(*kw))
                    .count();
                entry.score = Some(matched as f64 / keywords.len() as f64);
            }

            entries.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        } else {
            entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        }

        entries.truncate(limit);
        Ok(entries)
    }

    async fn delete(&self, key: &str) -> anyhow::Result<bool> {
        let content = self.read_file().await?;
        let entries = Self::parse_entries(&content);

        let original_len = entries.len();
        let updated_entries: Vec<&MemoryEntry> = entries.iter().filter(|e| e.key != key).collect();

        if updated_entries.len() == original_len {
            return Ok(false);
        }

        let system_info = Self::extract_system_info_markdown(&content);
        let entries_md = Self::entries_to_markdown(updated_entries.iter().copied());
        let new_content = Self::compose_document(system_info.as_deref(), Some(&entries_md));

        self.write_file(&new_content).await?;
        Ok(true)
    }

    async fn persist(&self) -> anyhow::Result<()> {
        // Markdown backend writes immediately on add/delete,
        // so persist is a no-op (but verify the file exists).
        if self.path.exists() {
            Ok(())
        } else {
            // Create the file with header
            self.ensure_header().await
        }
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>> {
        let content = self.read_file().await?;
        let entries = Self::parse_entries(&content);
        Ok(entries.into_iter().find(|e| e.key == key))
    }

    async fn count(&self) -> anyhow::Result<usize> {
        let content = self.read_file().await?;
        Ok(Self::parse_entries(&content).len())
    }

    async fn health_check(&self) -> bool {
        if let Some(parent) = self.path.parent() {
            parent.exists()
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_memory() -> (TempDir, MarkdownMemory) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("memory.md");
        let mem = MarkdownMemory::new(path);
        (tmp, mem)
    }

    #[tokio::test]
    async fn test_name() {
        let (_tmp, mem) = temp_memory();
        assert_eq!(mem.name(), "markdown");
    }

    #[tokio::test]
    async fn test_health_check() {
        let (_tmp, mem) = temp_memory();
        assert!(mem.health_check().await);
    }

    #[tokio::test]
    async fn test_add_and_get() {
        let (_tmp, mem) = temp_memory();
        mem.add(
            "lang",
            "User prefers Rust",
            MemoryCategory::Core,
            Some("s1"),
        )
        .await
        .unwrap();

        let entry = mem.get("lang").await.unwrap();
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.key, "lang");
        assert_eq!(entry.content, "User prefers Rust");
        assert_eq!(entry.category, MemoryCategory::Core);
        assert_eq!(entry.session_id.as_deref(), Some("s1"));
    }

    #[tokio::test]
    async fn test_add_multiple() {
        let (_tmp, mem) = temp_memory();
        mem.add("k1", "Rust is fast", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.add("k2", "Python is slow", MemoryCategory::Daily, None)
            .await
            .unwrap();
        mem.add("k3", "Rust and safety", MemoryCategory::Conversation, None)
            .await
            .unwrap();

        assert_eq!(mem.count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_query_keyword() {
        let (_tmp, mem) = temp_memory();
        mem.add("k1", "Rust is fast", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.add("k2", "Python is slow", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.add("k3", "Rust and safety", MemoryCategory::Core, None)
            .await
            .unwrap();

        let results = mem.query("Rust", 10, None).await.unwrap();
        assert!(results.len() >= 2);
        assert!(results
            .iter()
            .all(|r| r.content.to_lowercase().contains("rust")));
    }

    #[tokio::test]
    async fn test_query_no_match() {
        let (_tmp, mem) = temp_memory();
        mem.add("k1", "Rust is great", MemoryCategory::Core, None)
            .await
            .unwrap();

        let results = mem.query("javascript", 10, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_query_by_key() {
        let (_tmp, mem) = temp_memory();
        mem.add(
            "nginx",
            "How to install nginx",
            MemoryCategory::Conversation,
            None,
        )
        .await
        .unwrap();
        mem.add("docker", "Docker setup", MemoryCategory::Conversation, None)
            .await
            .unwrap();

        let results = mem.query("nginx", 10, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "nginx");
    }

    #[tokio::test]
    async fn test_query_by_session() {
        let (_tmp, mem) = temp_memory();
        mem.add("k1", "Session 1 note", MemoryCategory::Core, Some("s1"))
            .await
            .unwrap();
        mem.add("k2", "Session 2 note", MemoryCategory::Core, Some("s2"))
            .await
            .unwrap();

        let results = mem.query("", 10, Some("s1")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "k1");
    }

    #[tokio::test]
    async fn test_query_limit() {
        let (_tmp, mem) = temp_memory();
        for i in 0..5 {
            mem.add(
                &format!("k{i}"),
                &format!("Entry {i}"),
                MemoryCategory::Core,
                None,
            )
            .await
            .unwrap();
        }

        let results = mem.query("", 2, None).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_delete() {
        let (_tmp, mem) = temp_memory();
        mem.add("k1", "Keep this", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.add("k2", "Delete this", MemoryCategory::Core, None)
            .await
            .unwrap();

        let removed = mem.delete("k2").await.unwrap();
        assert!(removed);
        assert_eq!(mem.count().await.unwrap(), 1);

        let entry = mem.get("k1").await.unwrap();
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.key, "k1");
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let (_tmp, mem) = temp_memory();
        let removed = mem.delete("nonexistent").await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_persist_creates_file() {
        let (_tmp, mem) = temp_memory();
        mem.persist().await.unwrap();
        assert!(mem.path.exists());
    }

    #[tokio::test]
    async fn test_persist_noop_when_exists() {
        let (_tmp, mem) = temp_memory();
        mem.add("k1", "test", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.persist().await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_empty_count() {
        let (_tmp, mem) = temp_memory();
        assert_eq!(mem.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_empty_query() {
        let (_tmp, mem) = temp_memory();
        let results = mem.query("anything", 10, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_file_is_valid_markdown() {
        let (_tmp, mem) = temp_memory();
        mem.add(
            "nginx",
            "User asked about nginx",
            MemoryCategory::Conversation,
            Some("s1"),
        )
        .await
        .unwrap();

        let content = fs::read_to_string(&mem.path).await.unwrap();
        assert!(content.starts_with("# Nano-Assistant Memory\n"));
        assert!(content.contains("## "));
        assert!(content.contains("- **Key**: nginx"));
        assert!(content.contains("- **Content**: User asked about nginx"));
        assert!(content.contains("- **Category**: conversation"));
        assert!(content.contains("- **Session**: s1"));
    }

    #[test]
    fn test_normalize_system_info_markdown_demotes_headings() {
        let content = "# System Information\n\n## System\n\n- **OS**: Arch Linux\n";
        let normalized = MarkdownMemory::normalize_system_info_markdown(content);
        assert!(!normalized.contains("# System Information"));
        assert!(normalized.contains("### System"));
        assert!(normalized.contains("**OS**: Arch Linux"));
    }

    #[test]
    fn test_upsert_system_info_markdown_preserves_existing_entries() {
        let entry = MemoryEntry {
            id: "id-1".into(),
            key: "nginx".into(),
            content: "nginx installed via pacman".into(),
            category: MemoryCategory::Core,
            timestamp: "2026-04-22 12:00:00".into(),
            session_id: None,
            score: None,
        };
        let existing = format!(
            "{DOCUMENT_HEADER}{}\n",
            MarkdownMemory::format_entry(&entry)
        );
        let updated = MarkdownMemory::upsert_system_info_markdown(
            &existing,
            "# System Information\n\n## System\n\n- **OS**: Arch Linux\n",
        );

        assert!(updated.contains(SYSTEM_INFO_START));
        assert!(updated.contains("## System Information"));
        assert!(updated.contains("### System"));
        assert!(updated.contains("- **Key**: nginx"));
    }

    #[tokio::test]
    async fn test_delete_preserves_system_info_block() {
        let (_tmp, mem) = temp_memory();
        let with_system = MarkdownMemory::upsert_system_info_markdown(
            "",
            "# System Information\n\n## System\n\n- **OS**: Arch Linux\n",
        );
        fs::write(&mem.path, with_system).await.unwrap();

        mem.add("k1", "Keep this", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.add("k2", "Delete this", MemoryCategory::Core, None)
            .await
            .unwrap();

        let removed = mem.delete("k2").await.unwrap();
        assert!(removed);

        let content = fs::read_to_string(&mem.path).await.unwrap();
        assert!(content.contains("## System Information"));
        assert!(content.contains("### System"));
        assert!(content.contains("- **Key**: k1"));
        assert!(!content.contains("- **Key**: k2"));
    }

    #[tokio::test]
    async fn test_query_ignores_system_info_sections() {
        let (_tmp, mem) = temp_memory();
        let with_system = MarkdownMemory::upsert_system_info_markdown(
            "",
            "# System Information\n\n## System\n\n- **OS**: Arch Linux\n",
        );
        fs::write(&mem.path, with_system).await.unwrap();

        mem.add(
            "nginx",
            "Nginx configured on port 80",
            MemoryCategory::Core,
            None,
        )
        .await
        .unwrap();

        let results = mem.query("Arch Linux", 10, None).await.unwrap();
        assert!(results.is_empty());

        let results = mem.query("nginx", 10, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "nginx");
    }

    #[tokio::test]
    async fn test_custom_category() {
        let (_tmp, mem) = temp_memory();
        mem.add(
            "note",
            "Custom note",
            MemoryCategory::Custom("project".to_string()),
            None,
        )
        .await
        .unwrap();

        let entry = mem.get("note").await.unwrap().unwrap();
        assert_eq!(
            entry.category,
            MemoryCategory::Custom("project".to_string())
        );
    }

    #[tokio::test]
    async fn test_chrono_now_format() {
        let ts = chrono_now();
        assert!(ts.len() >= 19);
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], " ");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
    }
}
