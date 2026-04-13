use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Memory categories for organizing entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCategory {
    Core,
    Daily,
    Conversation,
    Custom(String),
}

impl Serialize for MemoryCategory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for MemoryCategory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "core" => Self::Core,
            "daily" => Self::Daily,
            "conversation" => Self::Conversation,
            _ => Self::Custom(s),
        })
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core => write!(f, "core"),
            Self::Daily => write!(f, "daily"),
            Self::Conversation => write!(f, "conversation"),
            Self::Custom(name) => write!(f, "{name}"),
        }
    }
}

/// A single memory entry.
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub score: Option<f64>,
}

impl std::fmt::Debug for MemoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryEntry")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("content", &self.content)
            .field("category", &self.category)
            .field("timestamp", &self.timestamp)
            .field("score", &self.score)
            .finish()
    }
}

/// Core memory trait — implement for any persistence backend.
///
/// Simplified from ZeroClaw: removes namespace isolation, vector embeddings,
/// procedural memory, GDPR export, importance scoring, and superseded tracking.
#[async_trait]
pub trait Memory: Send + Sync {
    /// Backend name.
    fn name(&self) -> &str;

    /// Store a new memory entry.
    async fn add(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Query memories matching a text query, returning up to `limit` results.
    async fn query(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Delete a memory entry by key. Returns true if it existed.
    async fn delete(&self, key: &str) -> anyhow::Result<bool>;

    /// Persist any pending writes to durable storage.
    ///
    /// For in-memory backends this is a no-op. For file/database backends
    /// this flushes buffers and ensures durability.
    async fn persist(&self) -> anyhow::Result<()>;

    /// Retrieve a specific memory by key.
    async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>>;

    /// Count total stored memories.
    async fn count(&self) -> anyhow::Result<usize>;

    /// Health check — returns true if the backend is operational.
    async fn health_check(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_category_display() {
        assert_eq!(MemoryCategory::Core.to_string(), "core");
        assert_eq!(MemoryCategory::Daily.to_string(), "daily");
        assert_eq!(MemoryCategory::Conversation.to_string(), "conversation");
        assert_eq!(MemoryCategory::Custom("notes".into()).to_string(), "notes");
    }

    #[test]
    fn memory_category_serde_roundtrip() {
        let core = serde_json::to_string(&MemoryCategory::Core).unwrap();
        assert_eq!(core, "\"core\"");
        let parsed: MemoryCategory = serde_json::from_str(&core).unwrap();
        assert_eq!(parsed, MemoryCategory::Core);

        let custom = MemoryCategory::Custom("project".into());
        let json = serde_json::to_string(&custom).unwrap();
        let parsed: MemoryCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, custom);
    }

    #[test]
    fn memory_entry_roundtrip() {
        let entry = MemoryEntry {
            id: "id-1".into(),
            key: "lang".into(),
            content: "Rust".into(),
            category: MemoryCategory::Core,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: Some("s1".into()),
            score: Some(0.95),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: MemoryEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "id-1");
        assert_eq!(parsed.key, "lang");
        assert_eq!(parsed.content, "Rust");
        assert_eq!(parsed.category, MemoryCategory::Core);
        assert_eq!(parsed.session_id.as_deref(), Some("s1"));
        assert_eq!(parsed.score, Some(0.95));
    }
}
