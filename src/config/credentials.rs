//! Small, provider-specific credential files stored beside the normal config.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

pub fn deepseek_key_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("deepseek.key")
}

pub fn load_deepseek_key(config_path: &Path) -> anyhow::Result<Option<String>> {
    let path = deepseek_key_path(config_path);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("checking {}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("DeepSeek key path is not a regular file");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("restricting permissions on {}", path.display()))?;
        }
    }

    let value = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let key = value.trim();
    if key.is_empty() {
        Ok(None)
    } else if key.contains(['\r', '\n', '\0']) {
        bail!("DeepSeek key file contains invalid characters");
    } else {
        Ok(Some(key.to_owned()))
    }
}

pub fn save_deepseek_key(config_path: &Path, key: &str) -> anyhow::Result<()> {
    let key = key.trim();
    if key.is_empty() || key.len() > 8192 || key.contains(['\r', '\n', '\0']) {
        bail!("DeepSeek API key is empty or invalid");
    }
    let path = deepseek_key_path(config_path);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating protected key file in {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temp.write_all(key.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("saving {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_key_separately_with_restricted_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        save_deepseek_key(&config, "  sk-test-secret  ").unwrap();
        assert_eq!(
            load_deepseek_key(&config).unwrap().as_deref(),
            Some("sk-test-secret")
        );
        assert!(!fs::read_to_string(&config)
            .unwrap_or_default()
            .contains("sk-test-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(deepseek_key_path(&config))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn tightens_existing_key_file_permissions_before_reading() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        let path = deepseek_key_path(&config);
        fs::write(&path, "sk-local-existing").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            load_deepseek_key(&config).unwrap().as_deref(),
            Some("sk-local-existing")
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_read_a_symlink_as_a_secret_file() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        let outside = temp.path().join("outside");
        fs::write(&outside, "sk-not-a-secret-store").unwrap();
        symlink(&outside, deepseek_key_path(&config)).unwrap();
        assert!(load_deepseek_key(&config).is_err());
    }
}
