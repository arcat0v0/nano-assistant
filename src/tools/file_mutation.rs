use rig::tool::ToolExecutionError;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const PREVIEW_BYTES: usize = 64 * 1024;
static COMMIT_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    owner: u32,
    #[cfg(unix)]
    group: u32,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
                owner: metadata.uid(),
                group: metadata.gid(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                created: metadata.created().ok(),
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct FileSnapshot {
    identity: FileIdentity,
    bytes: u64,
    fingerprint: String,
}

pub(crate) struct PreparedFileMutation {
    requested: PathBuf,
    resolved: PathBuf,
    ancestors: Vec<(PathBuf, FileIdentity)>,
    baseline: Option<FileSnapshot>,
    content: String,
    old_preview: Option<String>,
    operation: &'static str,
    protected_backup: bool,
}

fn failure(error: impl std::fmt::Display) -> ToolExecutionError {
    ToolExecutionError::other(format!("File operation failed: {error}"))
}

async fn resolve(path: &Path) -> Result<PathBuf, ToolExecutionError> {
    if path.as_os_str().is_empty() {
        return Err(ToolExecutionError::invalid_args(
            "File path must not be empty",
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(failure)?.join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            Component::Normal(name) => {
                result.push(name);
                match tokio::fs::symlink_metadata(&result).await {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        result = tokio::fs::canonicalize(&result).await.map_err(failure)?;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(failure(error)),
                }
            }
            _ => result.push(component.as_os_str()),
        }
    }
    Ok(result)
}

async fn snapshot(
    file: &mut tokio::fs::File,
) -> Result<(FileSnapshot, Option<String>), ToolExecutionError> {
    let before = file.metadata().await.map_err(failure)?;
    if !before.is_file() {
        return Err(ToolExecutionError::invalid_args(
            "Target must be a regular file",
        ));
    }
    file.rewind().await.map_err(failure)?;
    let mut digest = Sha256::new();
    let mut preview = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut bytes = 0;
    loop {
        let count = file.read(&mut buffer).await.map_err(failure)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        digest.update(&buffer[..count]);
        if bytes <= PREVIEW_BYTES as u64 {
            preview.extend_from_slice(&buffer[..count]);
        }
    }
    let after = file.metadata().await.map_err(failure)?;
    if FileIdentity::from_metadata(&before) != FileIdentity::from_metadata(&after)
        || before.len() != bytes
        || after.len() != bytes
        || before.modified().ok() != after.modified().ok()
    {
        return Err(changed());
    }
    let preview = if bytes <= PREVIEW_BYTES as u64 {
        String::from_utf8(preview).ok()
    } else {
        None
    };
    Ok((
        FileSnapshot {
            identity: FileIdentity::from_metadata(&after),
            bytes,
            fingerprint: format!("{:x}", digest.finalize()),
        },
        preview,
    ))
}

fn changed() -> ToolExecutionError {
    ToolExecutionError::other("File target changed after preparation; nothing was written. Prepare and review the current target again.")
}

impl PreparedFileMutation {
    async fn prepare(path: &Path, content: &str) -> Result<Self, ToolExecutionError> {
        if path.as_os_str().is_empty() {
            return Err(ToolExecutionError::invalid_args(
                "File path must not be empty",
            ));
        }
        let requested = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().map_err(failure)?.join(path)
        };
        let resolved = resolve(path).await?;
        if super::is_protected_skill_path(&requested) || super::is_protected_skill_path(&resolved) {
            return Err(ToolExecutionError::permission_denied(format!(
                "Refusing to modify builtin skill source: {}",
                path.display()
            )));
        }
        let (baseline, old_preview) = match tokio::fs::symlink_metadata(&resolved).await {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(ToolExecutionError::invalid_args(
                        "Target must be a regular file",
                    ));
                }
                let mut options = tokio::fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                let mut file = options.open(&resolved).await.map_err(failure)?;
                let (snapshot, preview) = snapshot(&mut file).await?;
                (Some(snapshot), preview)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (None, Some(String::new()))
            }
            Err(error) => return Err(failure(error)),
        };
        let mut ancestors = Vec::new();
        let mut parent = resolved.parent();
        while let Some(directory) = parent {
            match tokio::fs::metadata(directory).await {
                Ok(metadata) => {
                    if !metadata.is_dir() {
                        return Err(ToolExecutionError::invalid_args(
                            "Parent must be a directory",
                        ));
                    }
                    ancestors.push((
                        directory.to_path_buf(),
                        FileIdentity::from_metadata(&metadata),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(failure(error)),
            }
            parent = directory.parent();
        }
        let protected_backup = if baseline.is_some() {
            match std::env::var_os("HOME") {
                Some(home) => {
                    resolved.starts_with(resolve(&PathBuf::from(home).join("Backup")).await?)
                }
                None => false,
            }
        } else {
            false
        };
        Ok(Self {
            requested,
            resolved,
            ancestors,
            protected_backup,
            operation: if baseline.is_some() {
                "overwrite"
            } else {
                "create"
            },
            baseline,
            content: content.to_owned(),
            old_preview,
        })
    }

    pub(crate) async fn write(path: &Path, content: &str) -> Result<Self, ToolExecutionError> {
        Self::prepare(path, content).await
    }

    pub(crate) async fn edit(
        path: &Path,
        old: &str,
        new: &str,
    ) -> Result<Self, ToolExecutionError> {
        if old.is_empty() {
            return Err(ToolExecutionError::invalid_args(
                "old_string must not be empty",
            ));
        }
        let mut prepared = Self::prepare(path, "").await?;
        if prepared.baseline.is_none() {
            return Err(ToolExecutionError::not_found("File not found"));
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let mut file = options.open(&prepared.resolved).await.map_err(failure)?;
        if !file.metadata().await.map_err(failure)?.is_file() {
            return Err(changed());
        }
        let mut original = String::new();
        file.read_to_string(&mut original).await.map_err(failure)?;
        if format!("{:x}", Sha256::digest(original.as_bytes()))
            != prepared
                .baseline
                .as_ref()
                .expect("existing target")
                .fingerprint
        {
            return Err(changed());
        }
        let matches = original.matches(old).count();
        if matches == 0 {
            return Err(ToolExecutionError::not_found(
                "old_string not found in file",
            ));
        }
        if matches > 1 {
            return Err(ToolExecutionError::invalid_args(format!(
                "old_string matches {matches} times; must match exactly once"
            )));
        }
        prepared.content = original.replacen(old, new, 1);
        prepared.operation = "edit";
        Ok(prepared)
    }

    pub(crate) fn evidence(&self) -> Value {
        let old_lines = self.old_preview.as_ref().map(|text| text.lines().count());
        json!({
            "requested_path": self.requested,
            "resolved_path": self.resolved,
            "exists": self.baseline.is_some(),
            "file_type": if self.baseline.is_some() { "regular" } else { "absent" },
            "baseline": self.baseline,
            "existing_ancestors": self.ancestors,
            "operation": self.operation,
            "exclusive_creation": self.baseline.is_none(),
            "proposed_bytes": self.content.len(),
            "proposed_fingerprint": format!("{:x}", Sha256::digest(self.content.as_bytes())),
            "old_line_count": old_lines,
            "new_line_count": self.content.lines().count(),
            "old_content_included": false,
            "protected_backup": self.protected_backup,
        })
    }

    pub(crate) fn preview(&self) -> String {
        let Some(old) = &self.old_preview else {
            return "Old content unavailable for preview (binary or larger than 64 KiB); full replacement requested.".into();
        };
        let text = format!("--- current\n{old}\n+++ proposed\n{}", self.content);
        if text.len() <= PREVIEW_BYTES {
            text
        } else {
            format!(
                "{}\n[local preview truncated at 64 KiB]",
                text.chars()
                    .scan(0usize, |bytes, ch| {
                        *bytes += ch.len_utf8();
                        Some((*bytes, ch))
                    })
                    .take_while(|(bytes, _)| *bytes <= PREVIEW_BYTES)
                    .map(|(_, ch)| ch)
                    .collect::<String>()
            )
        }
    }

    async fn revalidate_paths(&self) -> Result<(), ToolExecutionError> {
        if resolve(&self.requested).await? != self.resolved {
            return Err(changed());
        }
        for (path, identity) in &self.ancestors {
            let metadata = tokio::fs::metadata(path).await.map_err(|_| changed())?;
            if FileIdentity::from_metadata(&metadata) != *identity {
                return Err(changed());
            }
        }
        if super::is_protected_skill_path(&self.requested)
            || super::is_protected_skill_path(&self.resolved)
        {
            return Err(changed());
        }
        Ok(())
    }

    pub(crate) async fn commit(self) -> Result<(), ToolExecutionError> {
        let _gate = COMMIT_GATE.lock().await;
        self.revalidate_paths().await?;
        let mut file = if let Some(baseline) = &self.baseline {
            let mut options = tokio::fs::OpenOptions::new();
            options.read(true).write(true);
            #[cfg(unix)]
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            let mut file = options.open(&self.resolved).await.map_err(|_| changed())?;
            let (current, _) = snapshot(&mut file).await?;
            if current != *baseline {
                return Err(changed());
            }
            self.revalidate_paths().await?;
            file.rewind().await.map_err(failure)?;
            file
        } else {
            if tokio::fs::symlink_metadata(&self.resolved).await.is_ok() {
                return Err(changed());
            }
            if let Some(parent) = self.resolved.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(failure)?;
            }
            self.revalidate_paths().await?;
            tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.resolved)
                .await
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        changed()
                    } else {
                        failure(error)
                    }
                })?
        };
        file.write_all(self.content.as_bytes())
            .await
            .map_err(failure)?;
        file.set_len(self.content.len() as u64)
            .await
            .map_err(failure)?;
        file.flush().await.map_err(failure)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_target_appearing_after_preparation_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        let prepared = PreparedFileMutation::write(&path, "new").await.unwrap();
        assert_eq!(prepared.evidence()["exists"], false);
        tokio::fs::write(&path, "external").await.unwrap();
        assert!(prepared
            .commit()
            .await
            .unwrap_err()
            .to_string()
            .contains("changed"));
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "external");
    }

    #[tokio::test]
    async fn changed_content_blocks_write_but_identical_autosave_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        tokio::fs::write(&path, "before").await.unwrap();
        let prepared = PreparedFileMutation::write(&path, "after").await.unwrap();
        tokio::fs::write(&path, "before").await.unwrap();
        prepared.commit().await.unwrap();
        let prepared = PreparedFileMutation::edit(&path, "after", "edited")
            .await
            .unwrap();
        tokio::fs::write(&path, "external").await.unwrap();
        assert!(prepared.commit().await.is_err());
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "external");
    }

    #[tokio::test]
    async fn metadata_and_local_preview_do_not_export_old_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        tokio::fs::write(&path, "PRIVATE_OLD_CONTENT")
            .await
            .unwrap();
        let prepared = PreparedFileMutation::write(&path, "new").await.unwrap();
        assert!(!prepared
            .evidence()
            .to_string()
            .contains("PRIVATE_OLD_CONTENT"));
        assert!(prepared.preview().contains("PRIVATE_OLD_CONTENT"));
        prepared.commit().await.unwrap();
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "new");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_retarget_and_file_identity_change_block_commit() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one");
        let two = dir.path().join("two");
        let link = dir.path().join("link");
        tokio::fs::write(&one, "same").await.unwrap();
        tokio::fs::write(&two, "same").await.unwrap();
        symlink(&one, &link).unwrap();
        let prepared = PreparedFileMutation::write(&link, "new").await.unwrap();
        tokio::fs::remove_file(&link).await.unwrap();
        symlink(&two, &link).unwrap();
        assert!(prepared.commit().await.is_err());
        assert_eq!(tokio::fs::read_to_string(&two).await.unwrap(), "same");
        let prepared = PreparedFileMutation::write(&one, "new").await.unwrap();
        tokio::fs::rename(&two, &one).await.unwrap();
        assert!(prepared.commit().await.is_err());
    }

    #[tokio::test]
    async fn concurrent_creation_has_one_winner_and_preserves_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        let one = PreparedFileMutation::write(&path, "one").await.unwrap();
        let two = PreparedFileMutation::write(&path, "two").await.unwrap();
        let (one, two) = tokio::join!(one.commit(), two.commit());
        assert_ne!(one.is_ok(), two.is_ok());
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            if one.is_ok() { "one" } else { "two" }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn permission_denied_is_not_treated_as_a_new_file() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        tokio::fs::write(&path, "original").await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0))
            .await
            .unwrap();
        let result = PreparedFileMutation::write(&path, "new").await;
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();
        assert!(result.is_err());
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "original");
    }

    #[tokio::test]
    async fn invalid_edit_and_directory_targets_do_not_mutate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        tokio::fs::write(&path, "repeat repeat").await.unwrap();
        assert!(PreparedFileMutation::edit(&path, "repeat", "new")
            .await
            .is_err());
        assert!(PreparedFileMutation::edit(&path, "", "new").await.is_err());
        assert!(PreparedFileMutation::write(dir.path(), "new")
            .await
            .is_err());
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "repeat repeat"
        );
    }
}
