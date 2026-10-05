use rig::completion::Message;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub model: String,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub updated_at: String,
    pub model: String,
    pub preview: String,
    pub messages: usize,
}

pub struct ActiveSession {
    pub id: String,
    pub created_at: String,
}

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            dir: config_dir.join("sessions"),
        }
    }

    pub fn start(&self) -> ActiveSession {
        let stamp = timestamp_now();
        let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
        ActiveSession {
            id: format!("{stamp}-{suffix}"),
            created_at: stamp,
        }
    }

    pub fn save(
        &self,
        active: &ActiveSession,
        model: &str,
        messages: &[Message],
    ) -> io::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let session = StoredSession {
            id: active.id.clone(),
            created_at: active.created_at.clone(),
            updated_at: timestamp_now(),
            model: model.to_owned(),
            messages: messages.to_vec(),
        };
        let body = serde_json::to_vec(&session).map_err(io::Error::other)?;
        let path = self.dir.join(format!("{}.json", active.id));
        let staging = self.dir.join(format!("{}.json.tmp", active.id));
        std::fs::write(&staging, body)?;
        std::fs::rename(staging, path)?;
        Ok(())
    }

    pub fn list(&self) -> Vec<SessionMeta> {
        let mut metas: Vec<SessionMeta> = std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.ends_with(".json"))
                    })
                    .filter_map(|entry| Self::read_meta(&entry.path()))
                    .collect()
            })
            .unwrap_or_default();
        metas.sort_by(|a, b| b.id.cmp(&a.id));
        metas
    }

    pub fn load(&self, id: &str) -> io::Result<StoredSession> {
        let path = self.dir.join(format!("{id}.json"));
        let body = std::fs::read_to_string(path)?;
        serde_json::from_str(&body).map_err(io::Error::other)
    }

    fn read_meta(path: &Path) -> Option<SessionMeta> {
        let body = std::fs::read_to_string(path).ok()?;
        let session: StoredSession = serde_json::from_str(&body).ok()?;
        Some(SessionMeta {
            id: session.id,
            updated_at: session.updated_at,
            model: session.model,
            preview: preview_of(&session.messages),
            messages: session.messages.len(),
        })
    }
}

pub(crate) fn preview_of(messages: &[Message]) -> String {
    let text = messages
        .iter()
        .find_map(|message| match message {
            Message::User { content } => content.iter().find_map(|part| match part {
                rig::completion::message::UserContent::Text(text) => Some(text.text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_default();
    let squashed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const LIMIT: usize = 40;
    let mut chars = squashed.chars();
    let head: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn timestamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let (year, month, day) = crate::agent::prompt::days_to_date(secs / 86400);
    let day_secs = secs % 86400;
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        day_secs / 3600,
        (day_secs % 3600) / 60,
        day_secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SessionStore) {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        (temp, store)
    }

    #[test]
    fn save_list_load_roundtrip() {
        let (_temp, store) = store();
        let active = store.start();
        let messages = vec![
            Message::user("帮我安装一下 caddy，通过 podman"),
            Message::assistant("好的"),
        ];
        store.save(&active, "deepseek-flash", &messages).unwrap();

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, active.id);
        assert_eq!(list[0].model, "deepseek-flash");
        assert_eq!(list[0].messages, 2);
        assert_eq!(list[0].preview, "帮我安装一下 caddy，通过 podman");

        let loaded = store.load(&active.id).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.created_at, active.created_at);
        assert!(loaded.updated_at >= loaded.created_at);
    }

    #[test]
    fn empty_history_is_not_saved() {
        let (_temp, store) = store();
        let active = store.start();
        store.save(&active, "model", &[]).unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn list_orders_newest_first_and_skips_corrupt_files() {
        let (_temp, store) = store();
        let mut first = store.start();
        first.id = "20260101-000000-aaaaaaaa".into();
        let mut second = store.start();
        second.id = "20260202-000000-bbbbbbbb".into();
        store
            .save(&first, "m", &[Message::user("first question")])
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        store
            .save(&second, "m", &[Message::user("second question")])
            .unwrap();
        std::fs::write(store.dir.join("broken.json"), "{not json").unwrap();

        let list = store.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].preview, "second question");
        assert_eq!(list[1].preview, "first question");
    }

    #[test]
    fn preview_squashes_whitespace_and_truncates() {
        let (_temp, store) = store();
        let active = store.start();
        let long = format!("{}\n{}", "多".repeat(30), "字".repeat(30));
        store.save(&active, "m", &[Message::user(long)]).unwrap();
        let meta = &store.list()[0];
        assert!(!meta.preview.contains('\n'));
        assert!(meta.preview.ends_with('…'));
        assert!(meta.preview.chars().count() <= 41);
    }

    #[test]
    fn load_missing_session_errors() {
        let (_temp, store) = store();
        assert!(store.load("no-such-id").is_err());
    }

    #[test]
    fn session_ids_are_unique_within_the_same_second() {
        let (_temp, store) = store();
        let a = store.start();
        let b = store.start();
        assert_ne!(a.id, b.id);
    }
}
