use async_trait::async_trait;
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

mod archive;
mod system;

pub(crate) const RESULT_LIMIT: usize = 16 * 1024;
const TEXT_LIMIT: usize = 12 * 1024;
const SCAN_LIMIT: usize = 1024 * 1024;
const HASH_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct EvidenceContext {
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    pub protected_config: Option<PathBuf>,
    pub deadline: Instant,
    docker: DockerEnvironment,
    environment: Vec<(String, String)>,
    probe_stdout: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    probe_stderr: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone, Debug)]
struct DockerEnvironment {
    config: Option<PathBuf>,
    context: Option<String>,
    host: Option<String>,
}

impl EvidenceContext {
    pub(crate) fn new(
        cwd: PathBuf,
        home: Option<PathBuf>,
        protected_config: Option<PathBuf>,
        deadline: Instant,
    ) -> Self {
        let config = std::env::var_os("DOCKER_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".docker")));
        let config = config.map(|p| if p.is_absolute() { p } else { cwd.join(p) });
        let nonempty = |key| std::env::var(key).ok().filter(|v| !v.is_empty());
        let environment = [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
        ]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect();
        Self {
            cwd,
            home,
            protected_config,
            deadline,
            docker: DockerEnvironment {
                config,
                context: nonempty("DOCKER_CONTEXT"),
                host: nonempty("DOCKER_HOST"),
            },
            environment,
            probe_stdout: Default::default(),
            probe_stderr: Default::default(),
        }
    }
    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "evidence deadline exceeded",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PathRequest {
    pub operation: PathOperation,
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PathOperation {
    Stat,
    List,
    ReadText,
    Sha256,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveRequest {
    pub path: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContainerRuntime {
    Docker,
    Podman,
}
impl ContainerRuntime {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContainerKind {
    Container,
    Volume,
    Image,
    VolumeUsers,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContainerRequest {
    pub runtime: ContainerRuntime,
    pub kind: ContainerKind,
    pub name: String,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SystemdScope {
    User,
    System,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SystemdRequest {
    pub scope: SystemdScope,
    pub unit: String,
}
#[derive(Clone, Debug)]
pub(crate) enum EvidenceRequest {
    Path(PathRequest),
    Archive(ArchiveRequest),
    Container(ContainerRequest),
    Systemd(SystemdRequest),
}

impl EvidenceRequest {
    pub(crate) fn tool_name(&self) -> &'static str {
        match self {
            Self::Path(_) => "review_path",
            Self::Archive(_) => "review_archive",
            Self::Container(_) => "review_container",
            Self::Systemd(_) => "review_systemd",
        }
    }
    pub(crate) fn parse(tool: &str, arguments: &str) -> Result<Self, String> {
        let parse_error = |e: serde_json::Error| e.to_string();
        let request = match tool {
            "review_path" => Self::Path(serde_json::from_str(arguments).map_err(parse_error)?),
            "review_archive" => {
                Self::Archive(serde_json::from_str(arguments).map_err(parse_error)?)
            }
            "review_container" => {
                Self::Container(serde_json::from_str(arguments).map_err(parse_error)?)
            }
            "review_systemd" => {
                Self::Systemd(serde_json::from_str(arguments).map_err(parse_error)?)
            }
            _ => return Err("unknown evidence tool".into()),
        };
        request.validate()?;
        Ok(request)
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Path(r) => {
                validate_path(&r.path)?;
                if r.operation != PathOperation::ReadText
                    && (r.offset.is_some() || r.limit.is_some())
                {
                    return Err("offset/limit only apply to read_text".into());
                }
                if r.offset == Some(0) || r.limit == Some(0) || r.limit.is_some_and(|n| n > 400) {
                    return Err("invalid line range".into());
                }
            }
            Self::Archive(r) => validate_path(&r.path)?,
            Self::Container(r) => {
                let image = r.kind == ContainerKind::Image;
                let max = if image { 256 } else { 128 };
                if r.name.is_empty()
                    || r.name.len() > max
                    || !r.name.as_bytes()[0].is_ascii_alphanumeric()
                    || !r.name.bytes().all(|b| {
                        b.is_ascii_alphanumeric()
                            || b"_.-".contains(&b)
                            || (image && b"/:@+".contains(&b))
                    })
                {
                    return Err("invalid container resource name".into());
                }
            }
            Self::Systemd(r) => {
                if r.unit.is_empty()
                    || r.unit.len() > 256
                    || r.unit.starts_with('-')
                    || !r
                        .unit
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.@:-".contains(&b))
                    || ![
                        ".service",
                        ".socket",
                        ".timer",
                        ".target",
                        ".mount",
                        ".automount",
                        ".path",
                        ".slice",
                        ".scope",
                    ]
                    .iter()
                    .any(|suffix| r.unit.ends_with(suffix))
                {
                    return Err("invalid systemd unit".into());
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    let path = json!({"type":"string","minLength":1,"maxLength":4096});
    [
        ("review_path","Read bounded local path metadata, directory entries, text lines or a full digest. Never search for credentials.",json!({"type":"object","additionalProperties":false,"required":["operation","path"],"properties":{"operation":{"enum":["stat","list","read_text","sha256"]},"path":path,"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":400}},"allOf":[{"if":{"properties":{"operation":{"enum":["stat","list","sha256"]}}},"then":{"not":{"anyOf":[{"required":["offset"]},{"required":["limit"]}]}}}]})),
        ("review_archive","Inspect bounded ZIP/TAR/TAR.GZ member metadata and digests without extracting. Completeness is not proof of backup coverage.",json!({"type":"object","additionalProperties":false,"required":["path"],"properties":{"path":path}})),
        ("review_container","Inspect a selected local Docker or Podman runtime. Scope must match the proposed command; no runtime fallback.",json!({"type":"object","additionalProperties":false,"required":["runtime","kind","name"],"properties":{"runtime":{"enum":["docker","podman"]},"kind":{"enum":["container","volume","image","volume_users"]},"name":{"type":"string","minLength":1,"maxLength":256}},"allOf":[{"if":{"properties":{"kind":{"const":"image"}}},"then":{"properties":{"name":{"pattern":"^[A-Za-z0-9][A-Za-z0-9_./:@+-]{0,255}$"}}},"else":{"properties":{"name":{"maxLength":128,"pattern":"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$"}}}}]})),
        ("review_systemd","Read fixed local systemd state fields, not logs or configuration contents.",json!({"type":"object","additionalProperties":false,"required":["scope","unit"],"properties":{"scope":{"enum":["user","system"]},"unit":{"type":"string","minLength":1,"maxLength":256,"pattern":"^[A-Za-z0-9_.@:][A-Za-z0-9_.@:-]*\\.(service|socket|timer|target|mount|automount|path|slice|scope)$"}}})),
    ].into_iter().map(|(name,description,parameters)| ToolDefinition { name:name.into(),description:description.into(),parameters }).collect()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceStatus {
    Ok,
    NotFound,
    Unavailable,
    InvalidRequest,
    LimitExceeded,
    Changed,
    Redacted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct EvidenceResult {
    pub tool: String,
    pub status: EvidenceStatus,
    pub target: String,
    pub complete: bool,
    pub data: Value,
    #[serde(skip)]
    snapshot: Option<PathSnapshot>,
}
impl EvidenceResult {
    pub(crate) fn new(
        tool: impl Into<String>,
        status: EvidenceStatus,
        target: impl Into<String>,
        complete: bool,
        data: Value,
    ) -> Self {
        let mut result = Self {
            tool: tool.into(),
            status,
            target: target.into(),
            complete,
            data,
            snapshot: None,
        };
        result.bound();
        result
    }
    pub(crate) fn error(tool: &str, target: &str, status: EvidenceStatus, message: &str) -> Self {
        Self::new(
            tool,
            status,
            target,
            false,
            json!({"error":message.chars().take(1024).collect::<String>()}),
        )
    }
    fn bound(&mut self) {
        let mut dropped = 0usize;
        while serde_json::to_vec(self).map_or(true, |v| v.len() > RESULT_LIMIT) {
            self.complete = false;
            if let Some(object) = self.data.as_object_mut() {
                object.insert("truncated".into(), json!(true));
                if object.contains_key("archive_read_complete") {
                    object.insert("archive_read_complete".into(), json!(false));
                    object.insert("integrity_checked".into(), json!(false));
                }
                let array = object
                    .iter_mut()
                    .filter_map(|(_, v)| v.as_array_mut())
                    .max_by_key(|a| a.len());
                if let Some(array) = array.filter(|a| !a.is_empty()) {
                    array.pop();
                    dropped += 1;
                    object.insert("omitted_entries".into(), json!(dropped));
                    continue;
                }
            }
            let scope = self.data.get("scope").cloned();
            self.data = json!({"truncated":true,"error":"result exceeded serialization budget"});
            if let Some(scope) = scope {
                self.data["scope"] = scope;
            }
            while serde_json::to_vec(self).map_or(true, |v| v.len() > RESULT_LIMIT) {
                if !self.target.is_empty() {
                    self.target = self
                        .target
                        .chars()
                        .take(self.target.chars().count() / 2)
                        .collect();
                    self.data["target_truncated"] = json!(true);
                } else if !self.tool.is_empty() {
                    self.tool = self
                        .tool
                        .chars()
                        .take(self.tool.chars().count() / 2)
                        .collect();
                } else {
                    self.data =
                        json!({"truncated":true,"error":"result exceeded serialization budget"});
                }
            }
            break;
        }
    }
}
#[async_trait]
pub(crate) trait EvidenceCollector: Send + Sync {
    async fn collect(&self, request: &EvidenceRequest, context: &EvidenceContext)
        -> EvidenceResult;
    async fn revalidate(
        &self,
        results: &[EvidenceResult],
        context: &EvidenceContext,
    ) -> Vec<String>;
}
#[derive(Default)]
pub(crate) struct ProductionEvidenceCollector {
    executor: system::LocalProbeExecutor,
}
#[async_trait]
impl EvidenceCollector for ProductionEvidenceCollector {
    async fn collect(
        &self,
        request: &EvidenceRequest,
        context: &EvidenceContext,
    ) -> EvidenceResult {
        let target = match request {
            EvidenceRequest::Path(r) => &r.path,
            EvidenceRequest::Archive(r) => &r.path,
            EvidenceRequest::Container(r) => &r.name,
            EvidenceRequest::Systemd(r) => &r.unit,
        };
        if let Err(error) = request.validate() {
            return EvidenceResult::error(
                request.tool_name(),
                target,
                EvidenceStatus::InvalidRequest,
                &error,
            );
        }
        if context.check_deadline().is_err() {
            return EvidenceResult::error(
                request.tool_name(),
                target,
                EvidenceStatus::LimitExceeded,
                "deadline exceeded",
            );
        }
        match request {
            EvidenceRequest::Container(r) => {
                system::collect_container(&self.executor, r, context).await
            }
            EvidenceRequest::Systemd(r) => {
                system::collect_systemd(&self.executor, r, context).await
            }
            _ => {
                let request = request.clone();
                let context = context.clone();
                let tool = request.tool_name();
                let target = target.to_string();
                tokio::task::spawn_blocking(move || match request {
                    EvidenceRequest::Path(r) => collect_path(&r, &context),
                    EvidenceRequest::Archive(r) => archive::collect(&r, &context),
                    _ => unreachable!(),
                })
                .await
                .unwrap_or_else(|_| {
                    EvidenceResult::error(
                        tool,
                        &target,
                        EvidenceStatus::Unavailable,
                        "collector worker failed",
                    )
                })
            }
        }
    }
    async fn revalidate(
        &self,
        results: &[EvidenceResult],
        context: &EvidenceContext,
    ) -> Vec<String> {
        let snapshots: Vec<_> = results.iter().filter_map(|r| r.snapshot.clone()).collect();
        let context = context.clone();
        tokio::task::spawn_blocking(move || {
            snapshots
                .into_iter()
                .filter_map(|s| {
                    if context.check_deadline().is_err() {
                        return Some(format!(
                            "Could not revalidate {}: deadline exceeded",
                            s.requested.display()
                        ));
                    }
                    match s.validate() {
                        Ok(()) => None,
                        Err(_) => Some(format!(
                            "Path evidence changed or could not be revalidated: {}",
                            s.requested.display()
                        )),
                    }
                })
                .collect()
        })
        .await
        .unwrap_or_else(|_| vec!["Path evidence revalidation unavailable".into()])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
}
impl Identity {
    fn from(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mode: metadata.mode(),
        }
    }
}
#[derive(Clone, Debug)]
struct PathSnapshot {
    requested: PathBuf,
    canonical: Option<PathBuf>,
    identity: Option<Identity>,
    links: Vec<(PathBuf, Identity, PathBuf)>,
}
impl PathSnapshot {
    fn capture(requested: &Path) -> io::Result<Self> {
        let links = link_chain(requested)?;
        match std::fs::canonicalize(requested) {
            Ok(canonical) => {
                let identity = Identity::from(&std::fs::metadata(&canonical)?);
                Ok(Self {
                    requested: requested.into(),
                    canonical: Some(canonical),
                    identity: Some(identity),
                    links,
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self {
                requested: requested.into(),
                canonical: None,
                identity: None,
                links,
            }),
            Err(e) => Err(e),
        }
    }
    fn validate(&self) -> io::Result<()> {
        let current = Self::capture(&self.requested)?;
        if current.canonical != self.canonical
            || current.identity != self.identity
            || current.links != self.links
        {
            Err(io::Error::other("snapshot changed"))
        } else {
            Ok(())
        }
    }
}
fn link_chain(path: &Path) -> io::Result<Vec<(PathBuf, Identity, PathBuf)>> {
    let mut pending = path.to_path_buf();
    let mut links = Vec::new();
    for _ in 0..40 {
        let mut current = PathBuf::new();
        let mut found = false;
        let components: Vec<_> = pending.components().collect();
        for (index, component) in components.iter().enumerate() {
            current.push(component.as_os_str());
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    let target = std::fs::read_link(&current)?;
                    links.push((current.clone(), Identity::from(&metadata), target.clone()));
                    let mut next = if target.is_absolute() {
                        target
                    } else {
                        current.parent().unwrap_or(Path::new("/")).join(target)
                    };
                    for remaining in &components[index + 1..] {
                        next.push(remaining.as_os_str());
                    }
                    pending = next;
                    found = true;
                    break;
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(links),
                Err(e) => return Err(e),
            }
        }
        if !found {
            return Ok(links);
        }
    }
    Err(io::Error::other("too many symbolic links"))
}
fn validate_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || path.contains("://")
        || path.starts_with("file:")
    {
        Err("invalid local path".into())
    } else {
        Ok(())
    }
}
fn resolve(path: &str, context: &EvidenceContext) -> io::Result<PathBuf> {
    validate_path(path).map_err(io::Error::other)?;
    let path = if path == "~" {
        context
            .home
            .clone()
            .ok_or_else(|| io::Error::other("HOME unavailable"))?
    } else if let Some(rest) = path.strip_prefix("~/") {
        context
            .home
            .as_ref()
            .ok_or_else(|| io::Error::other("HOME unavailable"))?
            .join(rest)
    } else {
        PathBuf::from(path)
    };
    let path = if path.is_absolute() {
        path
    } else {
        context.cwd.join(path)
    };
    Ok(path)
}
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}
fn sensitive_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == ".env"
        || name.starts_with(".env.")
        || name == ".dev-secret"
        || name.starts_with("credentials")
        || [
            "auth.json",
            ".netrc",
            ".npmrc",
            ".pypirc",
            "id_rsa",
            "id_ed25519",
        ]
        .contains(&name.as_str())
        || [".key", ".p12", ".pfx"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}
fn forbidden_content(path: &Path, context: &EvidenceContext) -> bool {
    if ["/proc", "/sys", "/dev"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || path == Path::new("/etc/shadow")
        || path == Path::new("/etc/gshadow")
    {
        return true;
    }
    if path
        .components()
        .any(|c| sensitive_name(&c.as_os_str().to_string_lossy()))
    {
        return true;
    }
    let protected = context
        .protected_config
        .iter()
        .chain(context.docker.config.iter());
    for config in protected {
        if path == normalize(config)
            || path.starts_with(config)
            || std::fs::canonicalize(config).is_ok_and(|p| path == p || path.starts_with(p))
        {
            return true;
        }
    }
    if let Some(home) = &context.home {
        for name in [
            ".ssh",
            ".gnupg",
            ".aws",
            ".kube",
            "Develop/op_secrets",
            ".docker",
        ] {
            let protected = home.join(name);
            if path.starts_with(&protected)
                || std::fs::canonicalize(&protected).is_ok_and(|p| path.starts_with(p))
            {
                return true;
            }
        }
    }
    false
}
fn open_snapshot(snapshot: &PathSnapshot) -> io::Result<File> {
    let canonical = snapshot
        .canonical
        .as_ref()
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(canonical)?;
    if !file.metadata()?.is_file() || Some(Identity::from(&file.metadata()?)) != snapshot.identity {
        return Err(io::Error::other("not a stable regular file"));
    }
    snapshot.validate()?;
    Ok(file)
}
fn finish_snapshot(snapshot: &PathSnapshot, file: Option<&File>) -> io::Result<()> {
    if let Some(file) = file {
        if Some(Identity::from(&file.metadata()?)) != snapshot.identity {
            return Err(io::Error::other("opened file changed"));
        }
    }
    snapshot.validate()
}
fn metadata_json(metadata: &Metadata) -> Value {
    let kind = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        "special"
    };
    let mut value = json!({"type":kind,"size":metadata.len(),"mtime_unix_ns":metadata.modified().ok().and_then(|time|time.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string())});
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let object = value.as_object_mut().unwrap();
        for (key, v) in [
            ("dev", metadata.dev()),
            ("inode", metadata.ino()),
            ("mode", u64::from(metadata.mode())),
            ("uid", u64::from(metadata.uid())),
            ("gid", u64::from(metadata.gid())),
        ] {
            object.insert(key.into(), json!(v));
        }
    }
    value
}
fn io_result(tool: &str, target: &str, error: io::Error) -> EvidenceResult {
    let (status, message) = match error.kind() {
        io::ErrorKind::NotFound => (EvidenceStatus::NotFound, "path not found"),
        io::ErrorKind::PermissionDenied => (EvidenceStatus::Unavailable, "permission denied"),
        io::ErrorKind::TimedOut => (EvidenceStatus::LimitExceeded, "deadline exceeded"),
        _ => (EvidenceStatus::Unavailable, "local evidence unavailable"),
    };
    EvidenceResult::error(tool, target, status, message)
}
fn collect_path(request: &PathRequest, context: &EvidenceContext) -> EvidenceResult {
    let tool = "review_path";
    let path = match resolve(&request.path, context) {
        Ok(p) => p,
        Err(e) => return io_result(tool, &request.path, e),
    };
    let snapshot = match PathSnapshot::capture(&path) {
        Ok(s) => s,
        Err(e) => return io_result(tool, &request.path, e),
    };
    if snapshot.canonical.is_none() {
        if request.operation == PathOperation::Stat {
            if let Ok(metadata) = std::fs::symlink_metadata(&path) {
                let mut result = EvidenceResult::new(
                    tool,
                    EvidenceStatus::Ok,
                    &request.path,
                    true,
                    json!({"exists":true,"target_exists":false,"metadata":metadata_json(&metadata),"canonical_target":null,"links":snapshot.links.iter().map(|(path,_,target)|json!({"path":path,"target":target})).collect::<Vec<_>>()}),
                );
                result.snapshot = Some(snapshot);
                return result;
            }
        }
        let mut result = EvidenceResult::new(
            tool,
            EvidenceStatus::NotFound,
            &request.path,
            true,
            json!({"exists":false}),
        );
        result.snapshot = Some(snapshot);
        return result;
    }
    let canonical = snapshot.canonical.as_ref().unwrap();
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) => return io_result(tool, &request.path, e),
    };
    let mut data = json!({"requested":path,"canonical_target":canonical,"metadata":metadata_json(&metadata),"links":snapshot.links.iter().map(|(path,_,target)|json!({"path":path,"target":target})).collect::<Vec<_>>()});
    let mut complete = true;
    let mut status = EvidenceStatus::Ok;
    let mut opened = None;
    let outcome: io::Result<()> = (|| {
        context.check_deadline()?;
        match request.operation {
            PathOperation::Stat => {
                data["exists"] = json!(true);
                data["mounts"] = mount_facts(canonical, context);
            }
            PathOperation::List => {
                if !std::fs::metadata(canonical)?.is_dir() {
                    return Err(io::Error::other("not a directory"));
                }
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY);
                }
                let directory = options.open(canonical)?;
                if Some(Identity::from(&directory.metadata()?)) != snapshot.identity {
                    return Err(io::Error::other("directory changed"));
                }
                opened = Some(directory);
                let (mut entries, truncated) = list_directory(opened.as_ref().unwrap(), context)?;
                entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                complete = !truncated;
                data["entries"] = json!(entries);
                data["truncated"] = json!(truncated);
                data["collected_entries"] = json!(entries.len());
                data["total_at_least"] = json!(entries.len() + usize::from(truncated));
            }
            PathOperation::ReadText | PathOperation::Sha256 => {
                if forbidden_content(&path, context) || forbidden_content(canonical, context) {
                    status = EvidenceStatus::Redacted;
                    complete = false;
                    data["error"] = json!("sensitive or special path content is not available");
                    return Ok(());
                }
                let mut file = open_snapshot(&snapshot)?;
                if request.operation == PathOperation::Sha256 {
                    if file.metadata()?.len() > HASH_LIMIT {
                        status = EvidenceStatus::LimitExceeded;
                        complete = false;
                        data["error"] = json!("file exceeds 64 MiB digest limit");
                        return Ok(());
                    }
                    let mut digest = Sha256::new();
                    let mut buffer = [0u8; 32768];
                    let mut total = 0u64;
                    loop {
                        context.check_deadline()?;
                        let count = file.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        total += count as u64;
                        if total > HASH_LIMIT {
                            return Err(io::Error::other("digest limit exceeded"));
                        }
                        digest.update(&buffer[..count]);
                    }
                    data["sha256"] = json!(format!("{:x}", digest.finalize()));
                    data["hashed_bytes"] = json!(total);
                } else {
                    let (text, first, last, eof, truncated, redacted, private_key) = read_text(
                        &mut file,
                        request.offset.unwrap_or(1),
                        request.limit.unwrap_or(200),
                        context,
                    )?;
                    if private_key {
                        status = EvidenceStatus::Redacted;
                        complete = false;
                        data["redacted"] = json!(true);
                        data["error"] = json!("private key content withheld");
                    } else {
                        complete = eof && !truncated && !redacted;
                        data["text"] = json!(text);
                        data["first_line"] = json!(first);
                        data["last_line"] = json!(last);
                        data["eof"] = json!(eof);
                        data["truncated"] = json!(truncated);
                        data["redacted"] = json!(redacted);
                    }
                }
                opened = Some(file);
            }
        }
        Ok(())
    })();
    if let Err(e) = outcome {
        if finish_snapshot(&snapshot, opened.as_ref()).is_err() {
            return EvidenceResult::error(
                tool,
                &request.path,
                EvidenceStatus::Changed,
                "path changed during collection",
            );
        }
        return io_result(tool, &request.path, e);
    }
    if finish_snapshot(&snapshot, opened.as_ref()).is_err() {
        return EvidenceResult::error(
            tool,
            &request.path,
            EvidenceStatus::Changed,
            "path changed during collection",
        );
    }
    let mut result = EvidenceResult::new(tool, status, &request.path, complete, data);
    result.snapshot = Some(snapshot);
    result
}
fn read_text(
    file: &mut File,
    offset: usize,
    limit: usize,
    context: &EvidenceContext,
) -> io::Result<(String, usize, usize, bool, bool, bool, bool)> {
    let mut buffer = [0u8; 8192];
    let mut scanned = 0;
    let mut line = Vec::new();
    let mut output = String::new();
    let mut number = 1;
    let mut last = offset.saturating_sub(1);
    let mut eof = false;
    let mut truncated = false;
    let mut redacted = false;
    let end = offset.saturating_add(limit);
    'scan: loop {
        context.check_deadline()?;
        let count = file.read(&mut buffer[..(SCAN_LIMIT - scanned).min(8192)])?;
        if count == 0 {
            eof = scanned < SCAN_LIMIT;
            if !eof {
                truncated = true;
            }
            break;
        }
        scanned += count;
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                let scanned_text = std::str::from_utf8(&line)
                    .map_err(|_| io::Error::other("binary/non UTF-8 file"))?;
                if scanned_text.contains("-----BEGIN") && scanned_text.contains("PRIVATE KEY-----")
                {
                    return Ok((String::new(), offset, last, false, false, true, true));
                }
                if number >= offset {
                    if number >= end {
                        truncated = true;
                        break 'scan;
                    }
                    let text = std::str::from_utf8(&line)
                        .map_err(|_| io::Error::other("binary/non UTF-8 file"))?;
                    let safe = redact_line(text, &mut redacted);
                    if output.len() + safe.len() + 1 > TEXT_LIMIT {
                        truncated = true;
                        break 'scan;
                    }
                    output.push_str(&safe);
                    output.push('\n');
                    last = number;
                } else {
                    std::str::from_utf8(&line)
                        .map_err(|_| io::Error::other("binary/non UTF-8 file"))?;
                }
                number += 1;
                line.clear();
            } else {
                if line.len() >= TEXT_LIMIT {
                    truncated = true;
                    break 'scan;
                }
                line.push(*byte);
            }
        }
        if scanned >= SCAN_LIMIT {
            truncated = true;
            break;
        }
    }
    if eof && !line.is_empty() && number >= offset && number < end {
        let text =
            std::str::from_utf8(&line).map_err(|_| io::Error::other("binary/non UTF-8 file"))?;
        if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
            return Ok((String::new(), offset, last, false, false, true, true));
        }
        let safe = redact_line(text, &mut redacted);
        if output.len() + safe.len() <= TEXT_LIMIT {
            output.push_str(&safe);
            last = number;
        } else {
            truncated = true;
        }
    }
    Ok((output, offset, last, eof, truncated, redacted, false))
}
fn redact_line(text: &str, redacted: &mut bool) -> String {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(?:password|passwd|token|secret|api[_-]?key|private[_-]?key)["']?\s*[:=]|authorization["']?\s*[:=]?\s*["']?\s*(?:bearer|basic)\s+"#).unwrap()
    });
    if PATTERN.is_match(text) {
        *redacted = true;
        "[redacted sensitive line]".into()
    } else {
        text.into()
    }
}
#[cfg(unix)]
fn list_directory(file: &File, context: &EvidenceContext) -> io::Result<(Vec<Value>, bool)> {
    use std::os::fd::{AsRawFd, IntoRawFd};
    let descriptor = file.try_clone()?.into_raw_fd();
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe {
            libc::close(descriptor);
        }
        return Err(error);
    }
    struct DirectoryStream(*mut libc::DIR);
    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = DirectoryStream(stream);
    let mut entries = Vec::new();
    let mut truncated = false;
    loop {
        context.check_deadline()?;
        nix::errno::Errno::clear();
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            if let Some(code) = io::Error::last_os_error()
                .raw_os_error()
                .filter(|code| *code != 0)
            {
                return Err(io::Error::from_raw_os_error(code));
            }
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        if entries.len() == 128 {
            truncated = true;
            break;
        }
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                file.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let metadata = unsafe { metadata.assume_init() };
        let kind = match metadata.st_mode & libc::S_IFMT {
            libc::S_IFREG => "file",
            libc::S_IFDIR => "directory",
            libc::S_IFLNK => "symlink",
            _ => "special",
        };
        entries.push(
            json!({"name":name.to_string_lossy(),"type":kind,"size":metadata.st_size.max(0)}),
        );
    }
    Ok((entries, truncated))
}
#[cfg(not(unix))]
fn list_directory(_file: &File, _context: &EvidenceContext) -> io::Result<(Vec<Value>, bool)> {
    Err(io::Error::other(
        "descriptor-based directory enumeration unavailable",
    ))
}

fn mount_facts(target: &Path, context: &EvidenceContext) -> Value {
    #[cfg(target_os = "linux")]
    {
        let read = (|| {
            let mut file = File::open("/proc/self/mountinfo")?;
            let mut bytes = Vec::new();
            file.by_ref()
                .take((SCAN_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)?;
            context.check_deadline()?;
            if bytes.len() > SCAN_LIMIT {
                return Err(io::Error::other("mountinfo limit exceeded"));
            }
            String::from_utf8(bytes).map_err(io::Error::other)
        })();
        if let Ok(text) = read {
            let mut containing: Option<(usize, Value)> = None;
            let mut children = Vec::new();
            for line in text.lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() < 7 {
                    continue;
                }
                let point = fields[4]
                    .replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\012", "\n")
                    .replace("\\134", "\\");
                let path = Path::new(&point);
                let entry = json!({"mountpoint":point,"mount_id":fields[0],"device":fields[2]});
                if target.starts_with(path)
                    && containing
                        .as_ref()
                        .is_none_or(|(len, _)| point.len() > *len)
                {
                    containing = Some((point.len(), entry.clone()));
                }
                if path != target && path.starts_with(target) {
                    children.push(entry);
                }
            }
            return json!({"available":true,"containing":containing.map(|(_,v)|v),"children":children});
        }
    }
    let _ = (target, context);
    json!({"available":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(root: &Path) -> EvidenceContext {
        EvidenceContext::new(
            root.to_path_buf(),
            Some(root.to_path_buf()),
            None,
            Instant::now() + Duration::from_secs(5),
        )
    }
    #[tokio::test]
    async fn security_evidence_paths_digest_read_and_revalidate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data"), "hello\nworld\n").unwrap();
        let collector = ProductionEvidenceCollector::default();
        let ctx = context(dir.path());
        let req = EvidenceRequest::parse(
            "review_path",
            r#"{"operation":"read_text","path":"data","offset":2,"limit":1}"#,
        )
        .unwrap();
        let result = collector.collect(&req, &ctx).await;
        assert_eq!(result.status, EvidenceStatus::Ok);
        assert_eq!(result.data["text"], "world\n");
        assert!(collector
            .revalidate(std::slice::from_ref(&result), &ctx)
            .await
            .is_empty());
        std::fs::write(dir.path().join("data"), "changed").unwrap();
        assert!(!collector.revalidate(&[result], &ctx).await.is_empty());
        let missing = collector
            .collect(
                &EvidenceRequest::parse("review_path", r#"{"operation":"stat","path":"absent"}"#)
                    .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(missing.status, EvidenceStatus::NotFound);
        assert!(collector
            .revalidate(std::slice::from_ref(&missing), &ctx)
            .await
            .is_empty());
        std::fs::write(dir.path().join("absent"), "new").unwrap();
        assert!(!collector.revalidate(&[missing], &ctx).await.is_empty());
    }
    #[tokio::test]
    async fn security_evidence_sensitive_binary_and_bounded_lines() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let collector = ProductionEvidenceCollector::default();
        for (name, content, status) in [
            (".env", b"SECRET=yes".as_slice(), EvidenceStatus::Redacted),
            ("binary", b"\xff".as_slice(), EvidenceStatus::Unavailable),
            (
                "pem",
                b"-----BEGIN RSA PRIVATE KEY-----".as_slice(),
                EvidenceStatus::Redacted,
            ),
        ] {
            std::fs::write(dir.path().join(name), content).unwrap();
            let req = EvidenceRequest::Path(PathRequest {
                operation: PathOperation::ReadText,
                path: name.into(),
                offset: None,
                limit: None,
            });
            assert_eq!(collector.collect(&req, &ctx).await.status, status);
        }
        std::fs::write(dir.path().join("lines"), "password=hello\npublic=yes\n").unwrap();
        let req =
            EvidenceRequest::parse("review_path", r#"{"operation":"read_text","path":"lines"}"#)
                .unwrap();
        let result = collector.collect(&req, &ctx).await;
        assert!(!result.complete);
        assert!(!serde_json::to_string(&result).unwrap().contains("hello"));
        std::fs::write(dir.path().join("long"), "a".repeat(2 * 1024 * 1024)).unwrap();
        let req =
            EvidenceRequest::parse("review_path", r#"{"operation":"read_text","path":"long"}"#)
                .unwrap();
        let result = collector.collect(&req, &ctx).await;
        assert!(!result.complete);
        assert!(serde_json::to_vec(&result).unwrap().len() <= RESULT_LIMIT);
    }
    #[test]
    fn security_evidence_strict_requests() {
        for args in [
            r#"{"operation":"stat","path":"x","limit":2}"#,
            r#"{"operation":"read_text","path":"x","limit":401}"#,
            r#"{"operation":"stat","path":"https://host"}"#,
            r#"{"operation":"stat","path":"x","extra":true}"#,
        ] {
            assert!(EvidenceRequest::parse("review_path", args).is_err());
        }
        assert!(EvidenceRequest::parse(
            "review_container",
            r#"{"runtime":"docker","kind":"volume","name":"--host=bad"}"#
        )
        .is_err());
        assert!(EvidenceRequest::parse(
            "review_systemd",
            r#"{"scope":"system","unit":"*.service"}"#
        )
        .is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn security_evidence_links_fifo_directory_and_protected_config() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".ssh")).unwrap();
        std::fs::write(dir.path().join(".ssh/key"), "secret").unwrap();
        symlink(dir.path().join(".ssh/key"), dir.path().join("alias")).unwrap();
        let collector = ProductionEvidenceCollector::default();
        let ctx = context(dir.path());
        let result = collector
            .collect(
                &EvidenceRequest::parse("review_path", r#"{"operation":"sha256","path":"alias"}"#)
                    .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(result.status, EvidenceStatus::Redacted);
        let fifo = dir.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let result = collector
            .collect(
                &EvidenceRequest::parse(
                    "review_path",
                    r#"{"operation":"read_text","path":"fifo"}"#,
                )
                .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(result.status, EvidenceStatus::Unavailable);
        for n in 0..140 {
            std::fs::write(dir.path().join(format!("item{n}")), "").unwrap();
        }
        let result = collector
            .collect(
                &EvidenceRequest::parse("review_path", r#"{"operation":"list","path":"."}"#)
                    .unwrap(),
                &ctx,
            )
            .await;
        assert!(!result.complete);
        assert_eq!(result.data["entries"].as_array().unwrap().len(), 128);
    }
    #[tokio::test]
    async fn security_evidence_protected_paths_hash_limit_unicode_and_expired_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let collector = ProductionEvidenceCollector::default();
        let path = dir.path().join("application.toml");
        std::fs::write(&path, "ordinary=content").unwrap();
        let mut ctx = context(dir.path());
        ctx.protected_config = Some(path);
        let result = collector
            .collect(
                &EvidenceRequest::parse(
                    "review_path",
                    r#"{"operation":"read_text","path":"application.toml"}"#,
                )
                .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(result.status, EvidenceStatus::Redacted);
        let huge = File::create(dir.path().join("huge")).unwrap();
        huge.set_len(HASH_LIMIT + 1).unwrap();
        let result = collector
            .collect(
                &EvidenceRequest::parse("review_path", r#"{"operation":"sha256","path":"huge"}"#)
                    .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(result.status, EvidenceStatus::LimitExceeded);
        std::fs::write(dir.path().join("unicode"), "中文\n第二行\n").unwrap();
        let result = collector
            .collect(
                &EvidenceRequest::parse(
                    "review_path",
                    r#"{"operation":"read_text","path":"unicode","offset":2}"#,
                )
                .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(result.data["text"], "第二行\n");
        ctx.deadline = Instant::now();
        let result = collector
            .collect(
                &EvidenceRequest::parse("review_path", r#"{"operation":"stat","path":"unicode"}"#)
                    .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(result.status, EvidenceStatus::LimitExceeded);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn security_evidence_symlink_retarget_and_permission_not_found_distinction() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let collector = ProductionEvidenceCollector::default();
        std::fs::write(dir.path().join("one"), "one").unwrap();
        std::fs::write(dir.path().join("two"), "two").unwrap();
        symlink("one", dir.path().join("link")).unwrap();
        let result = collector
            .collect(
                &EvidenceRequest::parse("review_path", r#"{"operation":"stat","path":"link"}"#)
                    .unwrap(),
                &ctx,
            )
            .await;
        std::fs::remove_file(dir.path().join("link")).unwrap();
        symlink("two", dir.path().join("link")).unwrap();
        assert!(!collector.revalidate(&[result], &ctx).await.is_empty());
        if unsafe { libc::geteuid() } != 0 {
            std::fs::set_permissions(dir.path().join("one"), std::fs::Permissions::from_mode(0o0))
                .unwrap();
            let result = collector
                .collect(
                    &EvidenceRequest::parse(
                        "review_path",
                        r#"{"operation":"read_text","path":"one"}"#,
                    )
                    .unwrap(),
                    &ctx,
                )
                .await;
            assert_eq!(result.status, EvidenceStatus::Unavailable);
        }
    }
    #[tokio::test]
    async fn security_evidence_pem_skipped_header_and_quoted_authorization_are_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let collector = ProductionEvidenceCollector::default();
        std::fs::write(dir.path().join("notes"),"-----BEGIN RSA PRIVATE KEY-----\nbase64_private_material\n-----END RSA PRIVATE KEY-----\n").unwrap();
        let result = collector
            .collect(
                &EvidenceRequest::parse(
                    "review_path",
                    r#"{"operation":"read_text","path":"notes","offset":2}"#,
                )
                .unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(result.status, EvidenceStatus::Redacted);
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("base64_private_material"));
        std::fs::write(
            dir.path().join("headers"),
            "{\"Authorization\": \"Bearer abc123\"}\n'Authorization': 'Basic def456'\n",
        )
        .unwrap();
        let result = collector
            .collect(
                &EvidenceRequest::parse(
                    "review_path",
                    r#"{"operation":"read_text","path":"headers"}"#,
                )
                .unwrap(),
                &ctx,
            )
            .await;
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("abc123"));
        assert!(!encoded.contains("def456"));
        assert_eq!(result.data["redacted"], true);
    }
    #[test]
    fn security_evidence_encoded_control_target_fits_result_budget() {
        let result = EvidenceResult::error(
            "review_path",
            &"\u{1}".repeat(3000),
            EvidenceStatus::Unavailable,
            "unavailable",
        );
        assert!(serde_json::to_vec(&result).unwrap().len() <= RESULT_LIMIT);
        assert!(!result.complete);
    }
    #[cfg(unix)]
    #[test]
    fn security_evidence_directory_enumeration_remains_on_open_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let original = dir.path().join("tree");
        std::fs::create_dir(&original).unwrap();
        std::fs::write(original.join("original"), "real").unwrap();
        let handle = File::open(&original).unwrap();
        std::fs::rename(&original, dir.path().join("saved")).unwrap();
        std::fs::create_dir(&original).unwrap();
        std::fs::write(original.join("substituted"), "fake").unwrap();
        let (entries, truncated) = list_directory(&handle, &ctx).unwrap();
        assert!(!truncated);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "original");
    }
}
