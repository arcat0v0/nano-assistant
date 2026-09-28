use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nano_assistant::config::Config;
use serde_json::{json, Value};

struct RecordedRequest {
    method: String,
    path: String,
    headers: String,
    body: Option<Value>,
}

struct CatalogFixture {
    base_url: String,
    worker: thread::JoinHandle<Vec<RecordedRequest>>,
}

impl CatalogFixture {
    fn start(responses: Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "timed out waiting for model catalog request"
                            );
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("catalog listener failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let request = read_request(&mut stream);
                requests.push(request);
                write!(
                    stream,
                    "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    if status == 200 { "OK" } else { "Unauthorized" },
                    body.len(),
                )
                .unwrap();
                stream.flush().unwrap();
            }
            requests
        });
        Self { base_url, worker }
    }

    fn finish(self) -> Vec<RecordedRequest> {
        self.worker.join().unwrap()
    }
}

fn read_request(stream: &mut TcpStream) -> RecordedRequest {
    let mut data = Vec::new();
    let headers_end = loop {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "request ended before headers");
        data.extend_from_slice(&buffer[..count]);
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&data[..headers_end])
        .unwrap()
        .to_owned();
    let mut words = headers.split_whitespace();
    let method = words.next().unwrap().to_owned();
    let path = words.next().unwrap().to_owned();
    let content_length: usize = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    while data.len() - headers_end < content_length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "request ended before body");
        data.extend_from_slice(&buffer[..count]);
    }
    let body = (content_length != 0)
        .then(|| serde_json::from_slice(&data[headers_end..headers_end + content_length]).unwrap());
    RecordedRequest {
        method,
        path,
        headers,
        body,
    }
}

fn completion(model: &str) -> String {
    json!({
        "id": "local-completion",
        "object": "chat.completion",
        "created": 1,
        "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "Selected model answered."}, "finish_reason": "stop"}],
    })
    .to_string()
}

fn invoke(
    home: &Path,
    arguments: &[&str],
    input: Option<&str>,
    credential: Option<(&str, &str)>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_na"));
    command
        .args(arguments)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("NO_PROXY", "*")
        .env_remove("NA_API_KEY")
        .env_remove("NA_MODEL")
        .env_remove("NA_PROVIDER")
        .env_remove("OPENAI_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("MOONSHOT_API_KEY")
        .env_remove("GLM_API_KEY")
        .env_remove("MIMO_API_KEY")
        .env_remove("DASHSCOPE_API_KEY")
        .env_remove("ZAI_API_KEY")
        .env_remove("KIMI_API_KEY")
        .env_remove("QWEN_API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some((name, value)) = credential {
        command.env(name, value);
    }
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("CLI timed out: {}", String::from_utf8_lossy(&output.stderr));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "CLI failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn model_discover_uses_live_vendor_endpoints_and_authentication() {
    for (vendor, env_name, path, base_suffix, auth_header) in [
        (
            "deepseek",
            "DEEPSEEK_API_KEY",
            "/models",
            "",
            "authorization: Bearer local-key",
        ),
        (
            "kimi",
            "MOONSHOT_API_KEY",
            "/v1/models",
            "/v1",
            "authorization: Bearer local-key",
        ),
        (
            "glm",
            "GLM_API_KEY",
            "/v4/models",
            "/v4",
            "authorization: Bearer local-key",
        ),
        (
            "mimo",
            "MIMO_API_KEY",
            "/v1/models",
            "/v1",
            "api-key: local-key",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let response = json!({"object":"list","data":[{"id":format!("{vendor}-live")},{"id":format!("{vendor}-new")}]});
        let fixture = CatalogFixture::start(vec![(200, response.to_string())]);
        let url = format!("{}{}", fixture.base_url, base_suffix);
        let config_path = temp.path().join("assistant.toml");
        let output = invoke(
            temp.path(),
            &[
                "model",
                "--config-path",
                config_path.to_str().unwrap(),
                "discover",
                vendor,
                "--api-url",
                &url,
            ],
            None,
            Some((env_name, "local-key")),
        );
        assert_success(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("{vendor}-new")), "{stdout}");
        let requests = fixture.finish();
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, path);
        assert!(requests[0]
            .headers
            .to_ascii_lowercase()
            .contains(&auth_header.to_ascii_lowercase()));
    }
}

#[test]
fn qwen_discovery_reads_all_pages_and_only_queries_function_calling_models() {
    let temp = tempfile::tempdir().unwrap();
    let first: Vec<Value> = (0..100)
        .map(|index| json!({"model":format!("qwen-live-{index}")}))
        .collect();
    let fixture = CatalogFixture::start(vec![
        (200, json!({"output":{"total":101,"page_no":1,"page_size":100,"models":first}}).to_string()),
        (200, json!({"output":{"total":101,"page_no":2,"page_size":100,"models":[{"model":"qwen-latest"}]}}).to_string()),
    ]);
    let url = format!("{}/compatible-mode/v1", fixture.base_url);
    let config_path = temp.path().join("assistant.toml");
    let output = invoke(
        temp.path(),
        &[
            "model",
            "--config-path",
            config_path.to_str().unwrap(),
            "discover",
            "qwen",
            "--api-url",
            &url,
        ],
        None,
        Some(("DASHSCOPE_API_KEY", "local-key")),
    );
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("qwen-latest"));
    let requests = fixture.finish();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, "GET");
        assert!(
            request.path.starts_with("/api/v1/models?"),
            "{}",
            request.path
        );
        assert!(request.path.contains("providers=qwen"));
        assert!(request.path.contains("features=function-calling"));
        assert!(request
            .headers
            .to_ascii_lowercase()
            .contains("authorization: bearer local-key"));
    }
    assert!(requests[0].path.contains("page_no=1"));
    assert!(requests[1].path.contains("page_no=2"));
}

#[test]
fn tui_guides_addition_from_live_models_without_default_model_key() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = CatalogFixture::start(vec![
        (
            200,
            json!({"object":"list","data":[{"id":"deepseek-alpha"},{"id":"deepseek-zeta"}]})
                .to_string(),
        ),
        (200, completion("deepseek-zeta")),
    ]);
    let path = temp.path().join("assistant.toml");
    std::fs::write(&path, "[provider]\nprovider='openai'\nmodel='gpt-4o-mini'\n\n[behavior]\nstreaming=false\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n\n[hub]\nenabled=false\n").unwrap();
    let input = format!(
        "/model add deepseek\n\n{}\n2\nwork\ny\nhello\n/exit\n",
        fixture.base_url
    );
    let output = invoke(
        temp.path(),
        &["chat", "--config-path", path.to_str().unwrap()],
        Some(&input),
        Some(("DEEPSEEK_API_KEY", "local-key")),
    );
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Selected model answered."));
    let expected_url = fixture.base_url.clone();
    let requests = fixture.finish();
    assert_eq!(requests[0].path, "/models");
    assert_eq!(requests[1].path, "/chat/completions");
    assert_eq!(requests[1].body.as_ref().unwrap()["model"], "deepseek-zeta");
    let persisted: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(persisted.models.default.as_deref(), Some("work"));
    assert_eq!(persisted.models.profiles["work"].provider, "deepseek");
    assert_eq!(persisted.models.profiles["work"].model, "deepseek-zeta");
    assert_eq!(
        persisted.models.profiles["work"].api_key_env.as_deref(),
        Some("DEEPSEEK_API_KEY")
    );
    assert_eq!(
        persisted.models.profiles["work"].api_url.as_deref(),
        Some(expected_url.as_str())
    );
    assert!(!std::fs::read_to_string(path).unwrap().contains("local-key"));
}

#[test]
fn failed_online_discovery_leaves_tui_and_config_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = CatalogFixture::start(vec![(
        401,
        json!({"error":{"message":"unauthorized"}}).to_string(),
    )]);
    let path = temp.path().join("assistant.toml");
    let original = "[provider]\nprovider='openai'\nmodel='gpt-4o-mini'\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n";
    std::fs::write(&path, original).unwrap();
    let input = format!("/model add deepseek\n\n{}\n/exit\n", fixture.base_url);
    let output = invoke(
        temp.path(),
        &["chat", "--config-path", path.to_str().unwrap()],
        Some(&input),
        Some(("DEEPSEEK_API_KEY", "invalid-local-key")),
    );
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("401"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("invalid-local-key"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    assert_eq!(fixture.finish()[0].path, "/models");
}

#[test]
fn vendor_chat_uses_compatible_endpoints_and_documented_authentication() {
    for (vendor, key_env, base_suffix, path, expected_header) in [
        (
            "deepseek",
            "DEEPSEEK_API_KEY",
            "",
            "/chat/completions",
            "authorization: Bearer local-key",
        ),
        (
            "kimi",
            "MOONSHOT_API_KEY",
            "/v1",
            "/v1/chat/completions",
            "authorization: Bearer local-key",
        ),
        (
            "glm",
            "GLM_API_KEY",
            "/v4",
            "/v4/chat/completions",
            "authorization: Bearer local-key",
        ),
        (
            "mimo",
            "MIMO_API_KEY",
            "/v1",
            "/v1/chat/completions",
            "api-key: local-key",
        ),
        (
            "qwen",
            "DASHSCOPE_API_KEY",
            "/compatible-mode/v1",
            "/compatible-mode/v1/chat/completions",
            "authorization: Bearer local-key",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let fixture = CatalogFixture::start(vec![(200, completion("model-from-profile"))]);
        let path_config = temp.path().join("assistant.toml");
        std::fs::write(&path_config, format!("[provider]\nprovider='openai'\nmodel='gpt-4o-mini'\n\n[models.profiles.vendor]\nprovider='{vendor}'\nmodel='model-from-profile'\napi_url='{}{base_suffix}'\napi_key_env='{key_env}'\n\n[behavior]\nstreaming=false\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n\n[hub]\nenabled=false\n", fixture.base_url)).unwrap();
        let output = invoke(
            temp.path(),
            &[
                "chat",
                "--config-path",
                path_config.to_str().unwrap(),
                "--profile",
                "vendor",
                "Say hello",
            ],
            None,
            Some((key_env, "local-key")),
        );
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("Selected model answered."));
        let requests = fixture.finish();
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, path);
        assert_eq!(
            requests[0].body.as_ref().unwrap()["model"],
            "model-from-profile"
        );
        assert!(requests[0]
            .headers
            .to_ascii_lowercase()
            .contains(&expected_header.to_ascii_lowercase()));
    }
}

#[test]
fn tui_add_menu_lists_all_vendors_and_can_be_cancelled() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("assistant.toml");
    let original = "[provider]\nprovider='openai'\nmodel='gpt-4o-mini'\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n";
    std::fs::write(&path, original).unwrap();
    let output = invoke(
        temp.path(),
        &["chat", "--config-path", path.to_str().unwrap()],
        Some("/model add\nq\n/exit\n"),
        None,
    );
    assert_success(&output);
    let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
    for provider in ["deepseek", "kimi", "glm", "mimo", "qwen"] {
        assert!(text.contains(provider), "{provider} missing in {text}");
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

#[test]
fn discovery_without_vendor_key_fails_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("assistant.toml");
    let output = invoke(
        temp.path(),
        &[
            "model",
            "--config-path",
            path.to_str().unwrap(),
            "discover",
            "deepseek",
            "--api-url",
            "http://127.0.0.1:9",
        ],
        None,
        None,
    );
    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("DEEPSEEK_API_KEY"), "{message}");
    assert!(!message.contains("connecting"), "{message}");
    assert!(!path.exists());
}
