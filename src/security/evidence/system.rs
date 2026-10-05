use super::*;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

const STDOUT_LIMIT: usize = 1024 * 1024;
const STDERR_LIMIT: usize = 16 * 1024;
#[derive(Clone, Copy, Debug)]
enum ProbeProgram {
    Docker,
    Podman,
    Systemctl,
}
impl ProbeProgram {
    fn name(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
            Self::Systemctl => "systemctl",
        }
    }
}
#[derive(Default)]
pub(super) struct LocalProbeExecutor {
    #[cfg(test)]
    injected: Option<[Option<PathBuf>; 3]>,
}
#[derive(Debug)]
struct ProbeOutput {
    stdout: Vec<u8>,
    success: bool,
    code: Option<i32>,
}
struct RunningProbe {
    child: Child,
    #[cfg(unix)]
    group: Option<i32>,
}
impl Drop for RunningProbe {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}
impl LocalProbeExecutor {
    #[cfg(test)]
    fn for_test(
        docker: Option<PathBuf>,
        podman: Option<PathBuf>,
        systemctl: Option<PathBuf>,
    ) -> Self {
        Self {
            injected: Some([docker, podman, systemctl]),
        }
    }
    fn executable(&self, program: ProbeProgram) -> Result<PathBuf, String> {
        #[cfg(test)]
        if let Some(paths) = &self.injected {
            return paths[match program {
                ProbeProgram::Docker => 0,
                ProbeProgram::Podman => 1,
                ProbeProgram::Systemctl => 2,
            }]
            .clone()
            .ok_or_else(|| "program unavailable".into());
        }
        let mut seen = HashSet::new();
        for root in ["/usr/bin", "/bin"] {
            let Ok(path) = std::fs::canonicalize(Path::new(root).join(program.name())) else {
                continue;
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            if trusted_executable(&path) {
                return Ok(path);
            }
        }
        Err("trusted local program unavailable".into())
    }
    async fn execute(
        &self,
        program: ProbeProgram,
        args: &[String],
        context: &EvidenceContext,
    ) -> Result<ProbeOutput, String> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (program, args, context);
            return Err("system probes only available on Linux".into());
        }
        #[cfg(target_os = "linux")]
        {
            let path = self.executable(program)?;
            let mut command = Command::new(path);
            command
                .args(args)
                .current_dir(&context.cwd)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("LANG", "C.UTF-8")
                .env("LC_ALL", "C.UTF-8")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            if let Some(home) = &context.home {
                command.env("HOME", home);
            }
            for (key, value) in &context.environment {
                if key == "DBUS_SESSION_BUS_ADDRESS" {
                    if matches!(program, ProbeProgram::Systemctl) {
                        if !local_bus(value) {
                            return Err("nonlocal session bus unavailable".into());
                        }
                        command.env(key, value);
                    }
                } else {
                    command.env(key, value);
                }
            }
            command.process_group(0);
            let child = command
                .spawn()
                .map_err(|_| "local program could not be started".to_owned())?;
            let group = child.id().and_then(|id| i32::try_from(id).ok());
            let mut running = RunningProbe { child, group };
            let stdout = running.child.stdout.take().ok_or("stdout unavailable")?;
            let stderr = running.child.stderr.take().ok_or("stderr unavailable")?;
            let deadline = context
                .deadline
                .min(Instant::now() + Duration::from_secs(5));
            let work = async {
                let (stdout, _, status) = tokio::try_join!(
                    bounded_output(stdout, STDOUT_LIMIT, &context.probe_stdout),
                    bounded_output(stderr, STDERR_LIMIT, &context.probe_stderr),
                    async {
                        running
                            .child
                            .wait()
                            .await
                            .map_err(|_| "probe wait failed".to_owned())
                    }
                )?;
                Ok::<_, String>(ProbeOutput {
                    stdout,
                    success: status.success(),
                    code: status.code(),
                })
            };
            let outcome =
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), work).await;
            let result = match outcome {
                Ok(result) => result,
                Err(_) => Err("probe deadline exceeded".into()),
            };
            if result.is_err() {
                if let Some(group) = running.group {
                    unsafe {
                        libc::kill(-group, libc::SIGKILL);
                    }
                }
                let _ = running.child.kill().await;
                let _ = running.child.wait().await;
            }
            result
        }
    }
}
async fn bounded_output<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    total: &std::sync::atomic::AtomicUsize,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| "probe output read failed".to_owned())?;
        if count == 0 {
            return Ok(bytes);
        }
        let previous = total.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
        if previous.saturating_add(count) > limit {
            return Err("probe output limit exceeded".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}
fn trusted_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
            return false;
        }
        for ancestor in path.ancestors() {
            let Ok(metadata) = std::fs::metadata(ancestor) else {
                return false;
            };
            if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                return false;
            }
        }
        true
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}
fn local_bus(value: &str) -> bool {
    let Some(address) = value.strip_prefix("unix:") else {
        return false;
    };
    let mut endpoint = false;
    for part in address.split(',') {
        if let Some(path) = part.strip_prefix("path=") {
            if !path.starts_with('/') || endpoint {
                return false;
            }
            endpoint = true;
        } else if let Some(name) = part.strip_prefix("abstract=") {
            if name.is_empty() || endpoint {
                return false;
            }
            endpoint = true;
        } else if let Some(guid) = part.strip_prefix("guid=") {
            if guid.len() != 32 || !guid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return false;
            }
        } else {
            return false;
        }
    }
    endpoint && !value.chars().any(char::is_control) && !value.contains(';')
}
fn failed(tool: &str, target: &str, error: &str) -> EvidenceResult {
    EvidenceResult::error(
        tool,
        target,
        if error.contains("limit") || error.contains("deadline") {
            EvidenceStatus::LimitExceeded
        } else {
            EvidenceStatus::Unavailable
        },
        error,
    )
}
fn check_output(output: ProbeOutput) -> Result<Vec<u8>, String> {
    if output.success {
        Ok(output.stdout)
    } else {
        Err(format!(
            "local query failed (exit code {})",
            output
                .code
                .map_or_else(|| "signal".into(), |n| n.to_string())
        ))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct DockerEndpoint {
    config: PathBuf,
    context: Option<String>,
    endpoint: String,
    socket: PathBuf,
    identity: Identity,
}
fn valid_context(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 256
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}
async fn docker_endpoint(
    executor: &LocalProbeExecutor,
    context: &EvidenceContext,
) -> Result<DockerEndpoint, String> {
    let config = context
        .docker
        .config
        .as_ref()
        .ok_or("Docker config directory unavailable")?
        .clone();
    if !config.is_absolute() {
        return Err("Docker config directory must be absolute".into());
    }
    let config_arg = config
        .to_str()
        .ok_or("Docker config directory is not UTF-8")?
        .to_owned();
    let mut name = context.docker.context.clone();
    let endpoint = if name.is_none() && context.docker.host.is_some() {
        context.docker.host.clone().unwrap()
    } else {
        if name.is_none() {
            let output = executor
                .execute(
                    ProbeProgram::Docker,
                    &[
                        "--config".into(),
                        config_arg.clone(),
                        "context".into(),
                        "show".into(),
                    ],
                    context,
                )
                .await?;
            let bytes = check_output(output)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| "invalid Docker context output")?;
            name = Some(text.trim().to_owned());
        }
        let context_name = name.as_ref().unwrap();
        if !valid_context(context_name) {
            return Err("invalid Docker context name".into());
        }
        let output = executor
            .execute(
                ProbeProgram::Docker,
                &[
                    "--config".into(),
                    config_arg,
                    "context".into(),
                    "inspect".into(),
                    "--".into(),
                    context_name.clone(),
                ],
                context,
            )
            .await?;
        let bytes = check_output(output)?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "invalid Docker context metadata")?;
        let array = value.as_array().ok_or("invalid Docker context metadata")?;
        if array.len() != 1 {
            return Err("ambiguous Docker context metadata".into());
        }
        array[0]
            .get("Endpoints")
            .and_then(|v| v.get("docker"))
            .and_then(|v| v.get("Host"))
            .and_then(Value::as_str)
            .ok_or("Docker context endpoint unavailable")?
            .to_owned()
    };
    let path = endpoint
        .strip_prefix("unix://")
        .ok_or("remote Docker endpoints are unavailable")?;
    if !path.starts_with('/')
        || path.contains(['?', '#', '\0'])
        || path.chars().any(char::is_control)
    {
        return Err("invalid local Docker endpoint".into());
    }
    let socket = std::fs::canonicalize(path).map_err(|_| "Docker socket unavailable")?;
    let metadata = std::fs::metadata(&socket).map_err(|_| "Docker socket unavailable")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_socket() {
            return Err("Docker endpoint is not a Unix socket".into());
        }
    }
    #[cfg(not(unix))]
    {
        return Err("Unix socket unavailable".into());
    }
    Ok(DockerEndpoint {
        config,
        context: name,
        endpoint: format!("unix://{}", socket.display()),
        socket,
        identity: Identity::from(&metadata),
    })
}
#[derive(Clone)]
struct RuntimeScope {
    runtime: ContainerRuntime,
    docker: Option<DockerEndpoint>,
    prefix: Vec<String>,
}
impl RuntimeScope {
    async fn freeze(
        executor: &LocalProbeExecutor,
        runtime: ContainerRuntime,
        context: &EvidenceContext,
    ) -> Result<Self, String> {
        match runtime {
            ContainerRuntime::Docker => {
                let docker = docker_endpoint(executor, context).await?;
                let prefix = vec![
                    "--config".into(),
                    docker
                        .config
                        .to_str()
                        .ok_or("non UTF-8 Docker config")?
                        .into(),
                    "--host".into(),
                    docker.endpoint.clone(),
                ];
                Ok(Self {
                    runtime,
                    docker: Some(docker),
                    prefix,
                })
            }
            ContainerRuntime::Podman => Ok(Self {
                runtime,
                docker: None,
                prefix: vec!["--remote=false".into()],
            }),
        }
    }
    fn program(&self) -> ProbeProgram {
        match self.runtime {
            ContainerRuntime::Docker => ProbeProgram::Docker,
            ContainerRuntime::Podman => ProbeProgram::Podman,
        }
    }
    fn json(&self) -> Value {
        #[cfg(unix)]
        let uid = Some(unsafe { libc::geteuid() });
        #[cfg(not(unix))]
        let uid: Option<u32> = None;
        json!({"runtime":self.runtime.as_str(),"effective_uid":uid,"endpoint":self.docker.as_ref().map_or("local",|d|d.endpoint.as_str()),"context":self.docker.as_ref().and_then(|d|d.context.as_ref())})
    }
    async fn query(
        &self,
        executor: &LocalProbeExecutor,
        arguments: Vec<String>,
        context: &EvidenceContext,
    ) -> Result<Vec<u8>, String> {
        let mut args = self.prefix.clone();
        args.extend(arguments);
        check_output(executor.execute(self.program(), &args, context).await?)
    }
    async fn unchanged(&self, executor: &LocalProbeExecutor, context: &EvidenceContext) -> bool {
        match &self.docker {
            Some(endpoint) => docker_endpoint(executor, context)
                .await
                .is_ok_and(|current| &current == endpoint),
            None => true,
        }
    }
}
fn string_field(object: &Value, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("inspect missing required field {key}"))
}
fn string_array(object: &Value, key: &str) -> Result<Value, String> {
    let value = object
        .get(key)
        .ok_or_else(|| format!("inspect missing required field {key}"))?;
    if value.is_null() {
        return Ok(json!([]));
    }
    let array = value
        .as_array()
        .ok_or_else(|| format!("invalid inspect field {key}"))?;
    if !array.iter().all(Value::is_string) {
        return Err(format!("invalid inspect field {key}"));
    }
    Ok(value.clone())
}
fn project_container(raw: &Value, runtime: ContainerRuntime) -> Result<Value, String> {
    let id = match runtime {
        ContainerRuntime::Docker => string_field(raw, "Id")?,
        ContainerRuntime::Podman => string_field(raw, "Id").or_else(|_| string_field(raw, "ID"))?,
    };
    let name = string_field(raw, "Name")?;
    let image_id = string_field(raw, "Image")?;
    let status = raw
        .get("State")
        .and_then(|v| v.get("Status"))
        .and_then(Value::as_str)
        .ok_or("inspect missing State.Status")?;
    let raw_mounts = raw
        .get("Mounts")
        .and_then(Value::as_array)
        .ok_or("inspect missing Mounts")?;
    let mut mounts = Vec::new();
    for mount in raw_mounts {
        let kind = string_field(mount, "Type")?;
        let name = if kind == "volume" {
            Some(string_field(mount, "Name")?)
        } else {
            mount.get("Name").and_then(Value::as_str).map(str::to_owned)
        };
        let source = string_field(mount, "Source")?;
        let destination = string_field(mount, "Destination")?;
        let rw = mount
            .get("RW")
            .and_then(Value::as_bool)
            .ok_or("inspect missing mount RW")?;
        mounts.push(
            json!({"type":kind,"name":name,"source":source,"destination":destination,"rw":rw}),
        );
    }
    Ok(
        json!({"id":id,"name":if runtime==ContainerRuntime::Docker{name.trim_start_matches('/')}else{&name},"image_id":image_id,"status":status,"mounts":mounts}),
    )
}
fn project_volume(raw: &Value, runtime: ContainerRuntime) -> Result<Value, String> {
    let scope = match runtime {
        ContainerRuntime::Docker => string_field(raw, "Scope")?,
        ContainerRuntime::Podman => string_field(raw, "Scope")?,
    };
    Ok(
        json!({"name":string_field(raw,"Name")?,"driver":string_field(raw,"Driver")?,"mountpoint":string_field(raw,"Mountpoint")?,"scope":scope}),
    )
}
fn project_image(raw: &Value, runtime: ContainerRuntime) -> Result<Value, String> {
    let id = match runtime {
        ContainerRuntime::Docker => string_field(raw, "Id")?,
        ContainerRuntime::Podman => string_field(raw, "Id").or_else(|_| string_field(raw, "ID"))?,
    };
    Ok(
        json!({"id":id,"repo_tags":string_array(raw,"RepoTags")?,"repo_digests":string_array(raw,"RepoDigests")?}),
    )
}
fn inspect_array(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "invalid inspect JSON")?;
    let values = value.as_array().ok_or("inspect result is not an array")?;
    if values.is_empty() {
        return Err("object not found or inspect returned no objects".into());
    }
    Ok(values.clone())
}
pub(super) async fn collect_container(
    executor: &LocalProbeExecutor,
    request: &ContainerRequest,
    context: &EvidenceContext,
) -> EvidenceResult {
    let tool = "review_container";
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (executor, context);
        return failed(tool, &request.name, "system probes only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        if let Err(error) = EvidenceRequest::Container(request.clone()).validate() {
            return EvidenceResult::error(
                tool,
                &request.name,
                EvidenceStatus::InvalidRequest,
                &error,
            );
        }
        let mut bounded_context = context.clone();
        bounded_context.deadline = context
            .deadline
            .min(Instant::now() + Duration::from_secs(5));
        bounded_context.probe_stdout = Default::default();
        bounded_context.probe_stderr = Default::default();
        let context = &bounded_context;
        let scope = match RuntimeScope::freeze(executor, request.runtime, context).await {
            Ok(scope) => scope,
            Err(error) => return failed(tool, &request.name, &error),
        };
        let mut data = json!({"scope":scope.json()});
        let query = async {
            if request.kind == ContainerKind::VolumeUsers {
                let bytes = scope
                    .query(
                        executor,
                        vec![
                            "ps".into(),
                            "--all".into(),
                            "--no-trunc".into(),
                            "--quiet".into(),
                        ],
                        context,
                    )
                    .await?;
                let text = std::str::from_utf8(&bytes).map_err(|_| "invalid container ID list")?;
                let mut ids = Vec::new();
                let mut truncated = false;
                let mut seen = HashSet::new();
                for line in text.lines() {
                    if line.len() != 64 || !line.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err("invalid full container ID".into());
                    }
                    if !seen.insert(line) {
                        return Err("duplicate container ID list".into());
                    }
                    if ids.len() == 64 {
                        truncated = true;
                        break;
                    }
                    ids.push(line.to_owned());
                }
                let mut containers = Vec::new();
                let mut failures = Vec::new();
                let mut inspected = HashSet::new();
                for batch in ids.chunks(16) {
                    let mut args = vec!["container".into(), "inspect".into(), "--".into()];
                    args.extend_from_slice(batch);
                    let values = match scope
                        .query(executor, args, context)
                        .await
                        .and_then(|bytes| inspect_array(&bytes))
                    {
                        Ok(values) => values,
                        Err(_) => {
                            failures.push("container inspection failed or object disappeared");
                            continue;
                        }
                    };
                    if values.len() != batch.len() {
                        failures.push("container inspection count differs from list");
                    }
                    for raw in values {
                        match project_container(&raw, request.runtime) {
                            Ok(mut container) => {
                                let id = container["id"].as_str().unwrap();
                                if !batch
                                    .iter()
                                    .any(|expected| expected.eq_ignore_ascii_case(id))
                                    || !inspected.insert(id.to_ascii_lowercase())
                                {
                                    failures
                                        .push("container inspection identity differs from list");
                                    continue;
                                }
                                let mounts: Vec<_> = container["mounts"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .filter(|m| {
                                        m["name"].as_str() == Some(&request.name)
                                            && m["type"] == "volume"
                                    })
                                    .cloned()
                                    .collect();
                                if !mounts.is_empty() {
                                    container["mounts"] = json!(mounts);
                                    containers.push(container);
                                }
                            }
                            Err(_) => failures.push("container inspection missing required fields"),
                        }
                    }
                }
                data["containers"] = json!(containers);
                data["listed_containers"] = json!(ids.len());
                data["truncated"] = json!(truncated);
                data["failures"] = json!(failures);
                Ok(!truncated && failures.is_empty())
            } else {
                let kind = match request.kind {
                    ContainerKind::Container => "container",
                    ContainerKind::Volume => "volume",
                    ContainerKind::Image => "image",
                    ContainerKind::VolumeUsers => unreachable!(),
                };
                let bytes = scope
                    .query(
                        executor,
                        vec![
                            kind.into(),
                            "inspect".into(),
                            "--".into(),
                            request.name.clone(),
                        ],
                        context,
                    )
                    .await?;
                let values = inspect_array(&bytes)?;
                if values.len() != 1 {
                    return Err("ambiguous inspect result".into());
                }
                data["object"] = match request.kind {
                    ContainerKind::Container => project_container(&values[0], request.runtime)?,
                    ContainerKind::Volume => project_volume(&values[0], request.runtime)?,
                    ContainerKind::Image => project_image(&values[0], request.runtime)?,
                    ContainerKind::VolumeUsers => unreachable!(),
                };
                Ok(true)
            }
        };
        let outcome: Result<bool, String> = query.await;
        let complete = match outcome {
            Ok(complete) => complete,
            Err(error) => {
                data["error"] = json!(error);
                false
            }
        };
        let unchanged = scope.unchanged(executor, context).await;
        if !unchanged {
            data["connection_changed"] = json!(true);
        }
        let status = if Instant::now() >= context.deadline {
            EvidenceStatus::LimitExceeded
        } else if !unchanged {
            EvidenceStatus::Changed
        } else if let Some(error) = data.get("error").and_then(Value::as_str) {
            if error.contains("limit") || error.contains("deadline") {
                EvidenceStatus::LimitExceeded
            } else {
                EvidenceStatus::Unavailable
            }
        } else {
            EvidenceStatus::Ok
        };
        EvidenceResult::new(
            tool,
            status,
            &request.name,
            complete && unchanged && status == EvidenceStatus::Ok,
            data,
        )
    }
}
pub(super) async fn collect_systemd(
    executor: &LocalProbeExecutor,
    request: &SystemdRequest,
    context: &EvidenceContext,
) -> EvidenceResult {
    let tool = "review_systemd";
    if let Err(error) = EvidenceRequest::Systemd(request.clone()).validate() {
        return EvidenceResult::error(tool, &request.unit, EvidenceStatus::InvalidRequest, &error);
    }
    let mut bounded_context = context.clone();
    bounded_context.deadline = context
        .deadline
        .min(Instant::now() + Duration::from_secs(5));
    bounded_context.probe_stdout = Default::default();
    bounded_context.probe_stderr = Default::default();
    let context = &bounded_context;
    let scope = match request.scope {
        SystemdScope::User => "--user",
        SystemdScope::System => "--system",
    };
    let properties = [
        "Id",
        "LoadState",
        "ActiveState",
        "SubState",
        "UnitFileState",
        "FragmentPath",
        "DropInPaths",
    ];
    let arguments = vec![
        scope.into(),
        "--no-pager".into(),
        "--no-ask-password".into(),
        "show".into(),
        format!("--property={}", properties.join(",")),
        "--".into(),
        request.unit.clone(),
    ];
    let bytes = match executor
        .execute(ProbeProgram::Systemctl, &arguments, context)
        .await
        .and_then(check_output)
    {
        Ok(bytes) => bytes,
        Err(error) => return failed(tool, &request.unit, &error),
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(_) => return failed(tool, &request.unit, "invalid systemd output"),
    };
    let mut fields = serde_json::Map::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return failed(tool, &request.unit, "invalid systemd property output");
        };
        if properties.contains(&key) && fields.insert(key.into(), json!(value)).is_some() {
            return failed(tool, &request.unit, "duplicate systemd property");
        }
    }
    if properties.iter().any(|key| !fields.contains_key(*key)) {
        return failed(
            tool,
            &request.unit,
            "systemd output missing required properties",
        );
    }
    let not_found = fields.get("LoadState").and_then(Value::as_str) == Some("not-found");
    EvidenceResult::new(
        tool,
        if not_found {
            EvidenceStatus::NotFound
        } else {
            EvidenceStatus::Ok
        },
        &request.unit,
        true,
        json!({"scope":if request.scope==SystemdScope::User{"user"}else{"system"},"properties":fields}),
    )
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn context(dir: &Path) -> EvidenceContext {
        EvidenceContext::new(
            dir.into(),
            Some(dir.into()),
            None,
            Instant::now() + Duration::from_secs(5),
        )
    }
    #[tokio::test]
    async fn security_evidence_container_both_runtimes_projection_and_volume_users() {
        let dir = tempfile::tempdir().unwrap();
        let socket =
            std::os::unix::net::UnixListener::bind(dir.path().join("docker.sock")).unwrap();
        let body = r#"case "$*" in
*"ps --all --no-trunc --quiet"*) printf '%064d\n%064d\n' 1 2;;
*) printf '%s' '[{"Id":"one","Name":"/running","Image":"image","State":{"Status":"running"},"Config":{"Env":["TOKEN=leak"]},"Mounts":[{"Type":"volume","Name":"shared","Source":"/data","Destination":"/srv","RW":true}]},{"Id":"two","Name":"stopped","Image":"image","State":{"Status":"exited"},"Mounts":[{"Type":"volume","Name":"shared","Source":"/data","Destination":"/srv","RW":false}]}]';;
esac"#;
        let body = body
            .replace("\"Id\":\"one\"", &format!("\"Id\":\"{:064x}\"", 1))
            .replace("\"Id\":\"two\"", &format!("\"Id\":\"{:064x}\"", 2));
        let path = script(dir.path(), "runtime", &body);
        let executor = LocalProbeExecutor::for_test(Some(path.clone()), Some(path), None);
        let mut ctx = context(dir.path());
        ctx.docker.host = Some(format!(
            "unix://{}",
            dir.path().join("docker.sock").display()
        ));
        ctx.docker.context = None;
        for runtime in [ContainerRuntime::Docker, ContainerRuntime::Podman] {
            let request = ContainerRequest {
                runtime,
                kind: ContainerKind::VolumeUsers,
                name: "shared".into(),
            };
            let result = collect_container(&executor, &request, &ctx).await;
            assert!(result.complete, "{result:?}");
            assert_eq!(result.data["containers"].as_array().unwrap().len(), 2);
            assert!(!serde_json::to_string(&result).unwrap().contains("leak"));
            assert_eq!(result.data["scope"]["runtime"], runtime.as_str());
        }
        drop(socket);
    }
    #[tokio::test]
    async fn security_evidence_remote_docker_never_falls_back_and_systemd_projection() {
        let dir = tempfile::tempdir().unwrap();
        let podman = script(dir.path(), "podman", "touch never; exit 0");
        let systemctl=script(dir.path(),"systemctl","printf 'Id=caddy.service\nLoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nFragmentPath=/etc/caddy.service\nDropInPaths=\nSECRET=leak\n'");
        let executor = LocalProbeExecutor::for_test(None, Some(podman), Some(systemctl));
        let mut ctx = context(dir.path());
        ctx.docker.host = Some("tcp://remote:2375".into());
        ctx.docker.context = None;
        let result = collect_container(
            &executor,
            &ContainerRequest {
                runtime: ContainerRuntime::Docker,
                kind: ContainerKind::Volume,
                name: "shared".into(),
            },
            &ctx,
        )
        .await;
        assert_eq!(result.status, EvidenceStatus::Unavailable);
        assert!(!dir.path().join("never").exists());
        let result = collect_systemd(
            &executor,
            &SystemdRequest {
                scope: SystemdScope::System,
                unit: "caddy.service".into(),
            },
            &ctx,
        )
        .await;
        assert!(result.complete);
        assert!(!serde_json::to_string(&result).unwrap().contains("leak"));
    }
    #[tokio::test]
    async fn security_evidence_executor_timeout_and_output_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let script = script(dir.path(), "hang", "sleep 20");
        let executor = LocalProbeExecutor::for_test(None, Some(script), None);
        let mut ctx = context(dir.path());
        ctx.deadline = Instant::now() + Duration::from_millis(30);
        assert!(executor
            .execute(ProbeProgram::Podman, &["--remote=false".into()], &ctx)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn security_evidence_docker_context_precedence_rootless_and_environment() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("rootless.sock");
        let _socket = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let body = format!(
            r#"case "$*" in
*"context show"*) printf 'rootless\n';;
*"context inspect -- rootless"*) printf '%s' '[{{"Endpoints":{{"docker":{{"Host":"unix://{}"}}}},"auths":{{"token":"never_send"}}}}]';;
*"volume inspect -- shared"*) [ -z "$DOCKER_HOST$DOCKER_CONTEXT$LD_PRELOAD$CONTAINER_HOST" ] || exit 8; printf '%s' '[{{"Name":"shared","Driver":"local","Mountpoint":"/data","Scope":"local","Labels":{{"secret":"never_send"}}}}]';;
*) exit 9;;
esac"#,
            socket_path.display()
        );
        let docker = script(dir.path(), "docker", &body);
        let executor = LocalProbeExecutor::for_test(Some(docker), None, None);
        let mut ctx = context(dir.path());
        ctx.docker.context = Some("rootless".into());
        ctx.docker.host = Some("tcp://wrong:2375".into());
        let request = ContainerRequest {
            runtime: ContainerRuntime::Docker,
            kind: ContainerKind::Volume,
            name: "shared".into(),
        };
        let result = collect_container(&executor, &request, &ctx).await;
        assert!(result.complete, "{result:?}");
        assert_eq!(result.data["scope"]["context"], "rootless");
        assert_eq!(
            result.data["scope"]["endpoint"],
            format!("unix://{}", socket_path.display())
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("never_send"));
        ctx.docker.context = None;
        ctx.docker.host = None;
        assert!(collect_container(&executor, &request, &ctx).await.complete);
    }
    #[tokio::test]
    async fn security_evidence_system_bad_json_missing_fields_and_local_bus() {
        let dir = tempfile::tempdir().unwrap();
        let bad = script(dir.path(), "bad", "printf '[{}]'");
        let executor = LocalProbeExecutor::for_test(None, Some(bad.clone()), Some(bad));
        let mut ctx = context(dir.path());
        let request = ContainerRequest {
            runtime: ContainerRuntime::Podman,
            kind: ContainerKind::Container,
            name: "caddy".into(),
        };
        let result = collect_container(&executor, &request, &ctx).await;
        assert_eq!(result.status, EvidenceStatus::Unavailable);
        assert!(!result.complete);
        ctx.environment
            .push(("DBUS_SESSION_BUS_ADDRESS".into(), "tcp:host=remote".into()));
        let result = collect_systemd(
            &executor,
            &SystemdRequest {
                scope: SystemdScope::User,
                unit: "caddy.service".into(),
            },
            &ctx,
        )
        .await;
        assert_eq!(result.status, EvidenceStatus::Unavailable);
        let missing = LocalProbeExecutor::for_test(None, None, None);
        assert!(missing
            .execute(ProbeProgram::Docker, &[], &ctx)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn security_evidence_system_output_limit_and_runtime_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let huge = script(
            dir.path(),
            "huge",
            "dd if=/dev/zero bs=1048576 count=2 2>/dev/null",
        );
        let executor = LocalProbeExecutor::for_test(None, Some(huge), None);
        let ctx = context(dir.path());
        assert!(executor
            .execute(ProbeProgram::Podman, &[], &ctx)
            .await
            .unwrap_err()
            .contains("limit"));
        for (runtime, name) in [
            (ContainerRuntime::Docker, "docker-user"),
            (ContainerRuntime::Podman, "podman-user"),
        ] {
            let body=format!("printf '%s' '[{{\"Id\":\"id\",\"Name\":\"{name}\",\"Image\":\"image\",\"State\":{{\"Status\":\"exited\"}},\"Mounts\":[]}}]'");
            let path = script(dir.path(), runtime.as_str(), &body);
            let executor = match runtime {
                ContainerRuntime::Docker => LocalProbeExecutor::for_test(Some(path), None, None),
                ContainerRuntime::Podman => LocalProbeExecutor::for_test(None, Some(path), None),
            };
            if runtime == ContainerRuntime::Podman {
                let result = collect_container(
                    &executor,
                    &ContainerRequest {
                        runtime,
                        kind: ContainerKind::Container,
                        name: "same".into(),
                    },
                    &ctx,
                )
                .await;
                assert_eq!(result.data["object"]["name"], name);
            } else {
                assert!(executor
                    .execute(ProbeProgram::Podman, &[], &ctx)
                    .await
                    .is_err());
            }
        }
    }
}
