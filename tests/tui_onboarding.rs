#![cfg(unix)]

use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nano_assistant::config::credentials::save_deepseek_key;
use nano_assistant::config::Config;
use nix::pty::{openpty, Winsize};

struct Request {
    method: String,
    path: String,
    headers: String,
    body: Option<serde_json::Value>,
}

struct ApiFixture {
    base_url: String,
    worker: thread::JoinHandle<Vec<Request>>,
}

struct CompletionFixture {
    base_url: String,
    worker: thread::JoinHandle<Request>,
}

impl CompletionFixture {
    fn start(response: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let response = response.to_owned();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "timed out waiting for completion"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("completion fixture failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request(&mut stream);
            let chunks = [
                serde_json::json!({
                    "id":"fixture",
                    "object":"chat.completion.chunk",
                    "created":1,
                    "model":"deepseek-flash",
                    "choices":[{"index":0,"delta":{"role":"assistant","content":response},"finish_reason":null}]
                }),
                serde_json::json!({
                    "id":"fixture",
                    "object":"chat.completion.chunk",
                    "created":1,
                    "model":"deepseek-flash",
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}]
                }),
            ];
            let mut body = chunks
                .iter()
                .map(|chunk| format!("data: {chunk}\n\n"))
                .collect::<String>();
            body.push_str("data: [DONE]\n\n");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            stream.flush().unwrap();
            request
        });
        Self { base_url, worker }
    }

    fn finish(self) -> Request {
        self.worker.join().unwrap()
    }
}

impl ApiFixture {
    fn start(responses: Vec<(u16, &'static str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(15);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "timed out waiting for API request"
                            );
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("API fixture failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let request = read_request(&mut stream);
                requests.push(request);
                write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
            requests
        });
        Self { base_url, worker }
    }

    fn finish(self) -> Vec<Request> {
        self.worker.join().unwrap()
    }
}

struct QueuedCompletionFixture {
    base_url: String,
    worker: thread::JoinHandle<Vec<Request>>,
}

impl QueuedCompletionFixture {
    fn start(responses: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for (content_type, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(15);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "timed out waiting for queued completion"
                            );
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("queued completion fixture failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                requests.push(read_request(&mut stream));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
            requests
        });
        Self { base_url, worker }
    }

    fn finish(self) -> Vec<Request> {
        self.worker.join().unwrap()
    }
}

fn queued_completion(content: &str) -> (&'static str, String) {
    (
        "application/json",
        serde_json::json!({
            "id":"fixture",
            "object":"chat.completion",
            "created":1,
            "model":"safety-test-model",
            "choices":[{
                "index":0,
                "message":{"role":"assistant","content":content},
                "finish_reason":"stop"
            }]
        })
        .to_string(),
    )
}

fn queued_stream(
    content: Option<&str>,
    call_id: &str,
    command: Option<&str>,
) -> (&'static str, String) {
    let delta = match command {
        Some(command) => serde_json::json!({
            "role":"assistant",
            "tool_calls":[{
                "index":0,
                "id":call_id,
                "type":"function",
                "function":{
                    "name":"shell",
                    "arguments":serde_json::json!({"command":command}).to_string()
                }
            }]
        }),
        None => serde_json::json!({"role":"assistant","content":content.unwrap()}),
    };
    let finish = if command.is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    let chunks = [
        serde_json::json!({
            "id":"fixture",
            "object":"chat.completion.chunk",
            "created":1,
            "model":"local-test-model",
            "choices":[{"index":0,"delta":delta,"finish_reason":null}]
        }),
        serde_json::json!({
            "id":"fixture",
            "object":"chat.completion.chunk",
            "created":1,
            "model":"local-test-model",
            "choices":[{"index":0,"delta":{},"finish_reason":finish}]
        }),
    ];
    let mut body = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    ("text/event-stream", body)
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "API client closed before request headers");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let content_length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "API client closed before complete request body");
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = (content_length > 0)
        .then(|| serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap());
    let mut first = headers.split_whitespace();
    Request {
        method: first.next().unwrap().to_owned(),
        path: first.next().unwrap().to_owned(),
        headers,
        body,
    }
}

struct Terminal {
    child: Child,
    master: File,
    control: File,
    output: Vec<u8>,
}

impl Terminal {
    fn start(home: &Path, config: &Path, deepseek_key: Option<&str>) -> Self {
        Self::start_with_size(home, config, deepseek_key, 100, 40)
    }

    fn start_with_size(
        home: &Path,
        config: &Path,
        deepseek_key: Option<&str>,
        columns: u16,
        rows: u16,
    ) -> Self {
        let pty = openpty(
            Some(&Winsize {
                ws_row: rows,
                ws_col: columns,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        let control = File::from(pty.slave.try_clone().unwrap());
        let slave = File::from(pty.slave);
        let stdin = slave.try_clone().unwrap();
        let stdout = slave.try_clone().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_na"));
        command
            .args(["chat", "--config-path", config.to_str().unwrap()])
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("TERM", "xterm")
            .env("NO_PROXY", "*")
            .env_remove("NA_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("DEEPSEEK_API_KEY")
            .env_remove("NA_MODEL")
            .env_remove("NA_PROVIDER")
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(slave));
        if let Some(key) = deepseek_key {
            command.env("DEEPSEEK_API_KEY", key);
        }
        let child = command.spawn().unwrap();
        let master = File::from(pty.master);
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        Self {
            child,
            master,
            control,
            output: Vec::new(),
        }
    }

    fn send(&mut self, input: &str) {
        self.master.write_all(input.as_bytes()).unwrap();
    }

    fn until(&mut self, expected: &str) {
        self.until_from(expected, 0);
    }

    fn until_from(&mut self, expected: &str, start: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&self.output[start..]).contains(expected) {
            let mut buffer = [0; 8192];
            match self.master.read(&mut buffer) {
                Ok(0) => panic!("terminal closed before {expected}: {}", self.text()),
                Ok(count) => self.output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for {expected}: {}",
                        self.text()
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("terminal read failed: {error}: {}", self.text()),
            }
        }
    }

    fn collect_for(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let mut buffer = [0; 8192];
            match self.master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => self.output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("terminal read failed: {error}"),
            }
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    fn resize(&self, columns: u16, rows: u16) {
        let size = Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
    }

    fn wait_success(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "terminal failed: {}", self.text());
                loop {
                    let mut buffer = [0; 8192];
                    match self.master.read(&mut buffer) {
                        Ok(count) if count > 0 => self.output.extend_from_slice(&buffer[..count]),
                        Ok(_) => break,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) => panic!("terminal read failed: {error}: {}", self.text()),
                    }
                }
                return;
            }
            if Instant::now() >= deadline {
                self.child.kill().unwrap();
                panic!("terminal did not exit: {}", self.text());
            }
            let mut buffer = [0; 8192];
            match self.master.read(&mut buffer) {
                Ok(count) if count > 0 => self.output.extend_from_slice(&buffer[..count]),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("terminal read failed: {error}: {}", self.text()),
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn terminal_restored(&self) -> bool {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(self.control.as_raw_fd(), &mut termios) } != 0 {
            return false;
        }
        termios.c_lflag & (libc::ICANON | libc::ECHO) == (libc::ICANON | libc::ECHO)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn config(path: &Path) -> PathBuf {
    fs::write(
        path,
        "[provider]\nprovider='openai'\nmodel='gpt-4o-mini'\napi_key='preserve-openai-key'\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n\n[hub]\nenabled=false\n",
    )
    .unwrap();
    path.to_path_buf()
}

fn config_with_deepseek_fixture(path: &Path, base_url: &str) -> PathBuf {
    let base = config(path);
    let mut source = fs::read_to_string(&base).unwrap();
    source.push_str(&format!(
        "\n[models]\ndefault='mock-deepseek'\n\n[models.profiles.mock-deepseek]\nprovider='deepseek'\nmodel='deepseek-flash'\napi_url='{base_url}'\napi_key_env='DEEPSEEK_API_KEY'\n"
    ));
    fs::write(&base, source).unwrap();
    base
}

fn configured_deepseek_chat(path: &Path, base_url: &str) -> PathBuf {
    fs::write(
        path,
        format!(
            "[provider]\nprovider='deepseek'\nmodel='deepseek-flash'\napi_key='fixture-only-key'\napi_url='{base_url}'\n\n[behavior]\nstreaming=true\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n\n[hub]\nenabled=false\n"
        ),
    )
    .unwrap();
    path.to_path_buf()
}

fn wait_for_exit(terminal: &mut Terminal) {
    terminal.send("/exit\r");
    terminal.wait_success();
}

#[test]
fn existing_non_deepseek_provider_skips_deepseek_onboarding() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = config(&temp.path().join("assistant.toml"));
    let mut terminal = Terminal::start(temp.path(), &config_path, None);
    terminal.until("CPU");
    terminal.until("❯ ");
    assert!(!terminal.text().contains("Connect DeepSeek"));
    assert!(!terminal.text().contains("API Key > "));
    wait_for_exit(&mut terminal);
    assert!(terminal.terminal_restored());

    let saved: Config = toml::from_str(&fs::read_to_string(config_path).unwrap()).unwrap();
    assert_eq!(saved.provider.provider.as_deref(), Some("openai"));
    assert_eq!(saved.provider.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(
        saved.provider.api_key.as_deref(),
        Some("preserve-openai-key")
    );
}

#[test]
fn first_run_masks_key_tests_it_and_preserves_other_provider_config() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = ApiFixture::start(vec![(200, r#"{"data":[{"id":"deepseek-flash"}]}"#)]);
    let config_path =
        config_with_deepseek_fixture(&temp.path().join("assistant.toml"), &fixture.base_url);
    let mut terminal = Terminal::start(temp.path(), &config_path, None);
    terminal.until("API Key > ");
    terminal.send("sk-local-secret\r");
    terminal.until("CPU");
    terminal.until("❯ ");
    terminal.until("deepseek-flash");
    assert!(!terminal.text().contains("sk-local-secret"));
    if let Some(path) = std::env::var_os("NA_TUI_CAPTURE_ANSI") {
        terminal.collect_for(Duration::from_millis(1200));
        fs::write(path, &terminal.output).unwrap();
    }
    wait_for_exit(&mut terminal);
    assert!(terminal.terminal_restored());
    let requests = fixture.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/models");
    assert!(requests[0]
        .headers
        .to_ascii_lowercase()
        .contains("authorization: bearer sk-local-secret"));

    let saved: Config = toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(saved.models.default.as_deref(), Some("deepseek"));
    assert_eq!(saved.provider.provider.as_deref(), Some("openai"));
    assert_eq!(saved.provider.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(
        saved.provider.api_key.as_deref(),
        Some("preserve-openai-key")
    );
    assert_eq!(saved.models.default.as_deref(), Some("deepseek"));
    let secret_path = config_path.parent().unwrap().join("deepseek.key");
    assert_eq!(fs::read_to_string(&secret_path).unwrap(), "sk-local-secret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&secret_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn existing_file_key_skips_first_run_setup_and_keeps_other_provider_config() {
    let temp = tempfile::tempdir().unwrap();
    let config_path =
        config_with_deepseek_fixture(&temp.path().join("assistant.toml"), "http://127.0.0.1:9");
    save_deepseek_key(&config_path, "sk-existing-local").unwrap();
    let mut terminal = Terminal::start(temp.path(), &config_path, None);
    terminal.until("CPU");
    terminal.until("❯ ");
    let output = terminal.text();
    assert!(!output.contains("Connect DeepSeek"));
    assert!(!output.contains("sk-existing-local"));
    wait_for_exit(&mut terminal);
    assert!(terminal.terminal_restored());

    let saved: Config = toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(saved.provider.provider.as_deref(), Some("openai"));
    assert_eq!(
        saved.provider.api_key.as_deref(),
        Some("preserve-openai-key")
    );
}

#[test]
fn rejected_key_can_be_edited_and_retried_without_leaking_either_value() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = ApiFixture::start(vec![
        (401, r#"{"error":{"message":"unauthorized"}}"#),
        (200, r#"{"data":[{"id":"deepseek-flash"}]}"#),
    ]);
    let config_path =
        config_with_deepseek_fixture(&temp.path().join("assistant.toml"), &fixture.base_url);
    let mut terminal = Terminal::start(temp.path(), &config_path, None);
    terminal.until("API Key > ");
    terminal.send("sk-invalid-local\r");
    terminal.until("[R retry] [E edit]");
    let edited_prompt = terminal.output.len();
    terminal.send("e\r");
    terminal.until_from("API Key > ", edited_prompt);
    terminal.send("sk-valid-local\r");
    terminal.until("CPU");
    terminal.until("❯ ");
    assert!(!terminal.text().contains("sk-invalid-local"));
    assert!(!terminal.text().contains("sk-valid-local"));
    wait_for_exit(&mut terminal);
    assert!(terminal.terminal_restored());
    let requests = fixture.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].headers.contains("Bearer sk-invalid-local"));
    assert!(requests[1].headers.contains("Bearer sk-valid-local"));
    let saved = fs::read_to_string(config_path.parent().unwrap().join("deepseek.key")).unwrap();
    assert_eq!(saved, "sk-valid-local");
}

#[test]
fn streamed_redraw_clears_wrapped_response_before_markdown_render() {
    let temp = tempfile::tempdir().unwrap();
    let response = "Hi! I'm your system steward on this Arch Linux box. What can I help you with today?\n\nA few things I can do right away:\n- help with system updates";
    let fixture = CompletionFixture::start(response);
    let config_path =
        configured_deepseek_chat(&temp.path().join("assistant.toml"), &fixture.base_url);
    let mut terminal = Terminal::start_with_size(temp.path(), &config_path, None, 80, 24);
    terminal.until("❯ ");
    terminal.send("hi\r");
    let request = fixture.finish();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    terminal.collect_for(Duration::from_millis(800));
    terminal.send("/exit\r");
    terminal.wait_success();

    let output = terminal.text();
    let markdown_redraw = output
        .find("\x1b[JHi!")
        .expect("streamed output should be replaced by rendered Markdown");
    assert!(
        output[..markdown_redraw].ends_with("\x1b[5A"),
        "80-column wrapped output occupies five terminal rows, but redraw moved by the wrong row count: {}",
        &output[markdown_redraw.saturating_sub(30)..markdown_redraw]
    );
}

#[test]
fn live_resource_updates_keep_unicode_input_and_adapt_after_resize() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = config(&temp.path().join("assistant.toml"));
    let mut terminal = Terminal::start(temp.path(), &config_path, Some("sk-preconfigured-local"));
    terminal.until("CPU");
    terminal.until("❯ ");
    terminal.send("\x1b[200~中文粘贴\x1b[201~");
    let initial = terminal.output.len();
    terminal.collect_for(Duration::from_millis(1300));
    assert!(terminal.text()[initial..].contains("中文粘贴"));
    assert!(terminal.text()[initial..].contains("CPU"));

    let checkpoint = terminal.output.len();
    terminal.resize(42, 20);
    terminal.send("\x0c");
    terminal.collect_for(Duration::from_millis(1300));
    let resized = terminal.text()[checkpoint..].to_owned();
    assert!(resized.contains("%"));
    assert!(resized.contains("中文粘贴"));
    terminal.send("\x01\x0b/exit\r");
    terminal.wait_success();
}

#[test]
fn escape_and_ctrl_c_during_masked_entry_exit_with_terminal_restored() {
    for abort in ["\x1b", "\x03"] {
        let temp = tempfile::tempdir().unwrap();
        let config_path =
            config_with_deepseek_fixture(&temp.path().join("assistant.toml"), "http://127.0.0.1:9");
        let mut terminal = Terminal::start(temp.path(), &config_path, None);
        terminal.until("API Key > ");
        terminal.send(abort);
        terminal.wait_success();
        assert!(terminal.terminal_restored());
        assert!(!config_path.parent().unwrap().join("deepseek.key").exists());
    }
}

#[test]
fn tui_auto_unknown_requires_current_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("review-marker.txt");
    let command = "printf reviewed > review-marker.txt";
    let main = QueuedCompletionFixture::start(vec![
        queued_stream(None, "call_deny", Some(command)),
        queued_stream(Some("First operation denied."), "", None),
        queued_stream(None, "call_allow", Some(command)),
        queued_stream(Some("Second operation completed."), "", None),
    ]);
    let reviewer = QueuedCompletionFixture::start(vec![
        queued_completion(
            r#"{"decision":"unknown","reason":"first action requires current approval"}"#,
        ),
        queued_completion(
            r#"{"decision":"unknown","reason":"second action requires current approval"}"#,
        ),
    ]);
    let config_path = temp.path().join("assistant.toml");
    fs::write(
        &config_path,
        format!(
            "[provider]\nprovider='compatible'\nmodel='local-test-model'\napi_url='{}'\n\n[behavior]\nstreaming=true\nmax_iterations=8\n\n[security]\nmode='auto'\nreview_profile='reviewer'\n\n[models.profiles.reviewer]\nprovider='compatible'\nmodel='safety-test-model'\napi_url='{}'\ntemperature=0\ntimeout_secs=1\n\n[skills]\nenabled=false\n\n[memory]\nenabled=false\n\n[hub]\nenabled=false\n",
            main.base_url, reviewer.base_url
        ),
    )
    .unwrap();
    let mut terminal = Terminal::start(temp.path(), &config_path, None);
    terminal.until("❯ ");
    let first_start = terminal.output.len();
    terminal.send("Write reviewed into review-marker.txt for the first task.\r");
    terminal.until_from("[y/N]", first_start);
    terminal.until_from("first action requires current approval", first_start);
    terminal.until_from("shell", first_start);
    terminal.until_from(command, first_start);
    assert!(!marker.exists());
    let denied_start = terminal.output.len();
    terminal.send("n\r");
    terminal.until_from("First operation denied.", denied_start);
    terminal.until_from("❯ ", denied_start);
    assert!(!marker.exists());

    let second_start = terminal.output.len();
    terminal.send("Write reviewed into review-marker.txt for the second task.\r");
    terminal.until_from("[y/N]", second_start);
    terminal.until_from("second action requires current approval", second_start);
    terminal.until_from("shell", second_start);
    terminal.until_from(command, second_start);
    assert!(!marker.exists());
    let allowed_start = terminal.output.len();
    terminal.send("y\r");
    terminal.until_from("Second operation completed.", allowed_start);
    terminal.until_from("❯ ", allowed_start);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "reviewed");
    wait_for_exit(&mut terminal);
    assert!(terminal.terminal_restored(), "{}", terminal.text());

    let main_requests = main.finish();
    assert_eq!(main_requests.len(), 4);
    for request in &main_requests {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/chat/completions");
        assert_eq!(request.body.as_ref().unwrap()["stream"], true);
    }
    let denied_result = main_requests[1].body.as_ref().unwrap()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("main model should receive the denied tool result");
    assert!(denied_result["content"]
        .to_string()
        .contains("Execution denied by user after safety review"));
    let allowed_result = main_requests[3].body.as_ref().unwrap()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .expect("main model should receive the executed tool result");
    assert!(!allowed_result["content"].to_string().contains("denied"));

    let review_requests = reviewer.finish();
    assert_eq!(review_requests.len(), 2);
    for (request, task) in review_requests.iter().zip(["first", "second"]) {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/chat/completions");
        let body = request.body.as_ref().unwrap();
        assert_eq!(body["model"], "safety-test-model");
        assert!(body.get("tools").is_none_or(|tools| {
            tools.is_null() || tools.as_array().is_some_and(Vec::is_empty)
        }));
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        let payload: serde_json::Value =
            serde_json::from_str(messages[1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(
            payload["user_request"],
            format!("Write reviewed into review-marker.txt for the {task} task.")
        );
        assert_eq!(payload["action"]["tool_name"], "shell");
        assert_eq!(payload["action"]["args"]["command"], command);
        assert_eq!(payload["action"]["resolved"]["command"], command);
    }
}
