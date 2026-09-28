use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nano_assistant::memory::{MarkdownMemory, Memory, MemoryCategory};
use serde_json::{json, Value};

struct ScriptedEndpoint {
    url: String,
    worker: thread::JoinHandle<Vec<Value>>,
}

impl ScriptedEndpoint {
    fn start(responses: Vec<(String, String)>) -> Self {
        Self::start_with_check(responses, |_, _| {})
    }

    fn start_with_check<F>(responses: Vec<(String, String)>, mut check: F) -> Self
    where
        F: FnMut(usize, &Value) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for (step, (content_type, body)) in responses.into_iter().enumerate() {
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "timed out waiting for model request"
                            );
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("model listener failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let (path, request) = read_request(&mut stream);
                assert_eq!(path, "/v1/chat/completions");
                check(step, &request);
                requests.push(request);
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
        Self { url, worker }
    }

    fn finish(self) -> Vec<Value> {
        self.worker.join().unwrap()
    }
}

struct LocalMcp {
    url: String,
    worker: thread::JoinHandle<Vec<Value>>,
}

impl LocalMcp {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let mut received = Vec::new();
            for expected in [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
            ] {
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "timed out waiting for MCP {expected}"
                            );
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("MCP listener failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let (path, request) = read_request(&mut stream);
                assert_eq!(path, "/mcp");
                assert_eq!(request["method"], expected);
                let result = match expected {
                    "initialize" => {
                        json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
                    }
                    "tools/list" => {
                        json!({"tools":[{"name":"echo","description":"Echoes a message from the local fixture","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]})
                    }
                    "tools/call" => {
                        json!({"content":[{"type":"text","text":"MCP returned the requested marker"}]})
                    }
                    _ => json!({}),
                };
                let body =
                    json!({"jsonrpc":"2.0","id":request.get("id"),"result":result}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).unwrap();
                stream.flush().unwrap();
                received.push(request);
            }
            received
        });
        Self { url, worker }
    }

    fn finish(self) -> Vec<Value> {
        self.worker.join().unwrap()
    }
}

fn read_request(stream: &mut TcpStream) -> (String, Value) {
    let mut data = Vec::new();
    let headers_end = loop {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "model request ended before headers");
        data.extend_from_slice(&buffer[..count]);
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        assert!(data.len() < 65536, "model request headers too large");
    };
    let headers = std::str::from_utf8(&data[..headers_end]).unwrap();
    let path = headers.split_whitespace().nth(1).unwrap().to_string();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse().unwrap())
        })
        .expect("model request must include a content length");
    while data.len() - headers_end < length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "model request ended before body");
        data.extend_from_slice(&buffer[..count]);
    }
    let body = serde_json::from_slice(&data[headers_end..headers_end + length]).unwrap();
    (path, body)
}

fn completion(content: Option<&str>, calls: Vec<Value>) -> (String, String) {
    let finish_reason = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    (
        "application/json".to_string(),
        json!({
            "id": "chatcmpl-local-test",
            "object": "chat.completion",
            "created": 1,
            "model": "local-test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content, "tool_calls": calls},
                "finish_reason": finish_reason
            }]
        })
        .to_string(),
    )
}

fn tool_call(id: &str, name: &str, args: Value) -> Value {
    json!({"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}})
}

fn stream_completion(content: &str) -> (String, String) {
    let chunks = [
        json!({"id":"chatcmpl-local-test","object":"chat.completion.chunk","created":1,"model":"local-test-model","choices":[{"index":0,"delta":{"role":"assistant","content":content},"finish_reason":null}]}),
        json!({"id":"chatcmpl-local-test","object":"chat.completion.chunk","created":1,"model":"local-test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
    ];
    sse(chunks.into_iter().collect())
}

fn stream_two_reads(first: &Path, second: &Path) -> (String, String) {
    let first_args = json!({"path":first}).to_string();
    let second_args = json!({"path":second}).to_string();
    let (first_start, first_end) = first_args.split_at(first_args.len() / 2);
    let (second_start, second_end) = second_args.split_at(second_args.len() / 2);
    let chunk = |calls: Value, finish: Option<&str>| json!({"id":"chatcmpl-local-test","object":"chat.completion.chunk","created":1,"model":"local-test-model","choices":[{"index":0,"delta":{"tool_calls":calls},"finish_reason":finish}]});
    sse(vec![
        chunk(
            json!([
                {"index":0,"id":"call_stream_a","type":"function","function":{"name":"file_read","arguments":first_start}},
                {"index":1,"id":"call_stream_b","type":"function","function":{"name":"file_read","arguments":second_start}}
            ]),
            None,
        ),
        chunk(
            json!([
                {"index":0,"function":{"arguments":first_end}},
                {"index":1,"function":{"arguments":second_end}}
            ]),
            None,
        ),
        chunk(json!([]), Some("tool_calls")),
    ])
}

fn sse(chunks: Vec<Value>) -> (String, String) {
    let mut data = String::new();
    for chunk in chunks {
        data.push_str("data: ");
        data.push_str(&chunk.to_string());
        data.push_str("\n\n");
    }
    data.push_str("data: [DONE]\n\n");
    ("text/event-stream".to_string(), data)
}

fn run_cli(home: &Path, config_path: &Path, input: Option<&str>, prompt: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_na"));
    command
        .arg("chat")
        .arg("--config-path")
        .arg(config_path)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("NO_PROXY", "*")
        .env_remove("NA_API_KEY")
        .env_remove("NA_PROVIDER")
        .env_remove("NA_MODEL")
        .env_remove("OPENAI_API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(prompt) = prompt {
        command.arg(prompt);
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
    let deadline = Instant::now() + Duration::from_secs(25);
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

fn config(
    home: &Path,
    url: &str,
    streaming: bool,
    memory: bool,
    security: &str,
) -> std::path::PathBuf {
    config_with_limit(home, url, streaming, memory, 100, security)
}

fn config_with_limit(
    home: &Path,
    url: &str,
    streaming: bool,
    memory: bool,
    max_messages: usize,
    security: &str,
) -> std::path::PathBuf {
    let path = home.join("assistant.toml");
    std::fs::write(
        &path,
        format!(
            "[provider]\nprovider = \"openai\"\nmodel = \"local-test-model\"\napi_key = \"local-fixture-key\"\napi_url = \"{url}\"\n\n[behavior]\nstreaming = {streaming}\nmax_iterations = 8\n\n[memory]\nenabled = {memory}\nmax_messages = {max_messages}\n\n[security]\n{security}\n\n[skills]\nenabled = false\n\n[hub]\nenabled = false\n"
        ),
    )
    .unwrap();
    path
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "CLI failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn tool_result<'a>(request: &'a Value, id: &str) -> &'a Value {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == id)
        .unwrap_or_else(|| panic!("missing model-visible result for {id}: {request}"))
}

#[test]
fn cli_reads_edits_writes_and_continues_after_each_tool() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original.txt");
    let generated = temp.path().join("nested/generated.txt");
    std::fs::write(&original, "alpha\n").unwrap();
    let endpoint = ScriptedEndpoint::start(vec![
        completion(
            None,
            vec![tool_call(
                "call_read",
                "file_read",
                json!({"path":original}),
            )],
        ),
        completion(
            None,
            vec![tool_call(
                "call_edit",
                "file_edit",
                json!({"path":original,"old_string":"alpha","new_string":"beta"}),
            )],
        ),
        completion(
            None,
            vec![tool_call(
                "call_write",
                "file_write",
                json!({"path":generated,"content":"saved by model\n"}),
            )],
        ),
        completion(Some("Files updated successfully."), vec![]),
    ]);
    let path = config(
        temp.path(),
        &endpoint.url,
        false,
        false,
        "mode = \"direct\"",
    );
    let output = run_cli(temp.path(), &path, None, Some("Update and save the files"));
    let requests = endpoint.finish();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Files updated successfully."));
    assert_eq!(std::fs::read_to_string(original).unwrap(), "beta\n");
    assert_eq!(
        std::fs::read_to_string(generated).unwrap(),
        "saved by model\n"
    );
    assert!(tool_result(&requests[1], "call_read")
        .to_string()
        .contains("alpha"));
    assert!(tool_result(&requests[2], "call_edit")
        .to_string()
        .contains("original.txt"));
    assert!(tool_result(&requests[3], "call_write")
        .to_string()
        .contains("generated.txt"));
}

#[test]
fn cli_recovers_from_duplicate_edit_with_a_corrected_tool_call() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("recover.txt");
    std::fs::write(&file, "repeat repeat\n").unwrap();
    let initial_file = file.clone();
    let endpoint = ScriptedEndpoint::start_with_check(
        vec![
            completion(
                None,
                vec![tool_call(
                    "call_failed",
                    "file_edit",
                    json!({"path":file,"old_string":"repeat","new_string":"incorrect"}),
                )],
            ),
            completion(
                None,
                vec![tool_call(
                    "call_corrected",
                    "file_edit",
                    json!({"path":file,"old_string":"repeat repeat","new_string":"fixed once"}),
                )],
            ),
            completion(Some("The corrected edit succeeded."), vec![]),
        ],
        move |step, _| {
            if step == 1 {
                assert_eq!(
                    std::fs::read_to_string(&initial_file).unwrap(),
                    "repeat repeat\n"
                );
            }
        },
    );
    let path = config(
        temp.path(),
        &endpoint.url,
        false,
        false,
        "mode = \"direct\"",
    );
    let output = run_cli(
        temp.path(),
        &path,
        None,
        Some("Fix the repeated text safely"),
    );
    let requests = endpoint.finish();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("The corrected edit succeeded."));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "fixed once\n");
    let failed = tool_result(&requests[1], "call_failed").to_string();
    assert!(
        failed.contains("2") && failed.contains("match"),
        "model did not receive ambiguity count: {failed}"
    );
    assert!(
        failed.contains("exactly once"),
        "model did not receive uniqueness requirement: {failed}"
    );
    let corrected = tool_result(&requests[2], "call_corrected").to_string();
    assert!(corrected.contains("recover.txt"));
    let assistant_ids: Vec<_> = requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten())
        .filter_map(|call| call["id"].as_str())
        .collect();
    assert!(assistant_ids.contains(&"call_failed"));
    assert!(assistant_ids.contains(&"call_corrected"));
}

#[test]
fn cli_denied_tool_has_no_filesystem_side_effect() {
    let temp = tempfile::tempdir().unwrap();
    let forbidden = temp.path().join("forbidden/deep/secret.txt");
    let command = format!(
        "mkdir -p '{}' && touch '{}'",
        forbidden.parent().unwrap().display(),
        forbidden.display()
    );
    let endpoint = ScriptedEndpoint::start(vec![
        completion(
            None,
            vec![tool_call(
                "call_denied",
                "shell",
                json!({"command":command}),
            )],
        ),
        completion(Some("The command was denied."), vec![]),
    ]);
    let path = config(
        temp.path(),
        &endpoint.url,
        false,
        false,
        "mode = \"whitelist\"\nwhitelist = [\"echo harmless\"]",
    );
    let output = run_cli(temp.path(), &path, None, Some("Run the forbidden command"));
    let requests = endpoint.finish();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("The command was denied."));
    assert!(!forbidden.exists());
    assert!(!temp.path().join("forbidden").exists());
    let result = tool_result(&requests[1], "call_denied")
        .to_string()
        .to_ascii_lowercase();
    assert!(
        result.contains("whitelist") || result.contains("denied"),
        "model did not receive denial: {result}"
    );
}

#[test]
fn cli_confirm_mode_user_rejection_does_not_execute_command() {
    let temp = tempfile::tempdir().unwrap();
    let forbidden = temp.path().join("never-created.txt");
    let command = format!("touch '{}'", forbidden.display());
    let endpoint = ScriptedEndpoint::start(vec![
        completion(
            None,
            vec![tool_call(
                "call_rejected",
                "shell",
                json!({"command":command}),
            )],
        ),
        completion(Some("The user rejected the command."), vec![]),
    ]);
    let path = config(
        temp.path(),
        &endpoint.url,
        false,
        false,
        "mode = \"confirm\"",
    );
    let output = run_cli(temp.path(), &path, Some("n\n"), Some("Try the command"));
    let requests = endpoint.finish();
    assert_success(&output);
    assert!(!forbidden.exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("The user rejected the command."));
    let result = tool_result(&requests[1], "call_rejected")
        .to_string()
        .to_ascii_lowercase();
    assert!(
        result.contains("denied") || result.contains("rejected"),
        "model did not receive user refusal: {result}"
    );
}

#[test]
fn cli_confirm_mode_names_rejected_file_write_without_creating_file() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("never-created.txt");
    let endpoint = ScriptedEndpoint::start(vec![
        completion(
            None,
            vec![tool_call(
                "call_rejected_write",
                "file_write",
                json!({"path":target,"content":"must not be written"}),
            )],
        ),
        completion(Some("The write was refused."), vec![]),
    ]);
    let config_path = config(
        temp.path(),
        &endpoint.url,
        false,
        false,
        "mode = \"confirm\"",
    );
    let output = run_cli(
        temp.path(),
        &config_path,
        Some("n\n"),
        Some("Write the file"),
    );
    let requests = endpoint.finish();
    assert_success(&output);
    assert!(!target.exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("file_write") && stderr.contains("never-created.txt"),
        "{stderr}"
    );
    assert!(!stderr.contains("<unknown>"), "{stderr}");
    assert!(tool_result(&requests[1], "call_rejected_write")
        .to_string()
        .contains("denied"));
}

#[test]
fn cli_streamed_file_edit_reports_tool_progress_only_on_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("progress.txt");
    std::fs::write(&target, "before\n").unwrap();
    let args = json!({"path":target,"old_string":"before","new_string":"after"});
    let endpoint = ScriptedEndpoint::start(vec![
        sse(vec![
            json!({"id":"progress-stream","object":"chat.completion.chunk","created":1,"model":"local-test-model","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_progress_edit","type":"function","function":{"name":"file_edit","arguments":args.to_string()}}]},"finish_reason":null}]}),
            json!({"id":"progress-stream","object":"chat.completion.chunk","created":1,"model":"local-test-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ]),
        stream_completion("Edit completed."),
    ]);
    let config_path = config(temp.path(), &endpoint.url, true, false, "mode = \"direct\"");
    let output = run_cli(temp.path(), &config_path, None, Some("Edit the file"));
    let requests = endpoint.finish();
    assert_success(&output);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "after\n");
    assert!(tool_result(&requests[1], "call_progress_edit")
        .to_string()
        .contains("progress.txt"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("file_edit") && stderr.contains("progress.txt"),
        "{stderr}"
    );
    assert!(stderr.contains("result"), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Edit completed."));
    assert!(!stdout.contains("call_progress_edit") && !stdout.contains("ToolResult"));
}

#[test]
fn cli_streamed_parallel_calls_preserve_both_results_and_ids() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("one.txt");
    let second = temp.path().join("two.txt");
    std::fs::write(&first, "first unique marker\n").unwrap();
    std::fs::write(&second, "second unique marker\n").unwrap();
    let endpoint = ScriptedEndpoint::start(vec![
        stream_two_reads(&first, &second),
        stream_completion("Both documents were read."),
    ]);
    let path = config(temp.path(), &endpoint.url, true, false, "mode = \"direct\"");
    let output = run_cli(temp.path(), &path, None, Some("Read both documents"));
    let requests = endpoint.finish();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Both documents were read."));
    assert_eq!(requests[0]["stream"], true);
    assert!(tool_result(&requests[1], "call_stream_a")
        .to_string()
        .contains("first unique marker"));
    assert!(tool_result(&requests[1], "call_stream_b")
        .to_string()
        .contains("second unique marker"));
    let assistant = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant" && message["tool_calls"].is_array())
        .expect("assistant tool-call history missing");
    let ids: Vec<_> = assistant["tool_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"call_stream_a"));
    assert!(ids.contains(&"call_stream_b"));
}

#[test]
fn cli_soft_message_limit_keeps_a_complete_tool_turn_for_next_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    std::fs::write(&source, "previous turn document marker\n").unwrap();
    let endpoint = ScriptedEndpoint::start(vec![
        completion(
            None,
            vec![tool_call(
                "call_previous_read",
                "file_read",
                json!({"path":source}),
            )],
        ),
        completion(Some("First answer complete."), vec![]),
        completion(Some("Second answer complete."), vec![]),
    ]);
    let path = config_with_limit(
        temp.path(),
        &endpoint.url,
        false,
        false,
        1,
        "mode = \"direct\"",
    );
    let output = run_cli(
        temp.path(),
        &path,
        Some("Read the first document\nWhat did you read earlier?\n/exit\n"),
        None,
    );
    let requests = endpoint.finish();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Second answer complete."));
    let history = requests[2]["messages"].as_array().unwrap();
    let prior_user = history
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"]
                    .to_string()
                    .contains("Read the first document")
        })
        .expect("prior user prompt was trimmed away");
    let prior_call = history
        .iter()
        .position(|message| {
            message["role"] == "assistant"
                && message["tool_calls"].as_array().is_some_and(|calls| {
                    calls.iter().any(|call| call["id"] == "call_previous_read")
                })
        })
        .expect("prior assistant tool call was trimmed away");
    let prior_result = history
        .iter()
        .position(|message| {
            message["role"] == "tool" && message["tool_call_id"] == "call_previous_read"
        })
        .expect("prior tool result was trimmed away");
    let current_user = history
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"]
                    .to_string()
                    .contains("What did you read earlier?")
        })
        .expect("current prompt was omitted");
    assert!(prior_user < prior_call && prior_call < prior_result && prior_result < current_user);
    assert!(tool_result(&requests[2], "call_previous_read")
        .to_string()
        .contains("previous turn document marker"));
    assert!(requests[2].to_string().contains("First answer complete."));
}

#[tokio::test]
async fn cli_memory_survives_history_clear_while_conversation_resets() {
    let temp = tempfile::tempdir().unwrap();
    let memory_path = temp.path().join(".config/nano-assistant/MEMORY.md");
    let memory = MarkdownMemory::new(memory_path.clone());
    memory
        .add(
            "keyboard",
            "I use a split keyboard",
            MemoryCategory::Core,
            None,
        )
        .await
        .unwrap();
    let endpoint = ScriptedEndpoint::start(vec![
        completion(Some("First reply from the model."), vec![]),
        completion(Some("Second reply from the model."), vec![]),
        completion(Some("Third reply from the model."), vec![]),
    ]);
    let path = config(temp.path(), &endpoint.url, false, true, "mode = \"direct\"");
    let output = run_cli(
        temp.path(),
        &path,
        Some("keyboard first\nkeyboard second\n/clear\nkeyboard after reset\n/exit\n"),
        None,
    );
    let requests = endpoint.finish();
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("First reply from the model."));
    assert!(stdout.contains("Second reply from the model."));
    assert!(stdout.contains("Third reply from the model."));
    assert!(requests[0].to_string().contains("I use a split keyboard"));
    assert!(requests[1]
        .to_string()
        .contains("First reply from the model."));
    assert!(requests[2].to_string().contains("I use a split keyboard"));
    assert!(!requests[2]
        .to_string()
        .contains("First reply from the model."));
    assert!(!requests[2]
        .to_string()
        .contains("Second reply from the model."));
    assert!(std::fs::read_to_string(memory_path)
        .unwrap()
        .contains("I use a split keyboard"));
}

#[test]
fn cli_discovers_activates_and_uses_local_mcp_tool() {
    let temp = tempfile::tempdir().unwrap();
    let mcp = LocalMcp::start();
    let endpoint = ScriptedEndpoint::start(vec![
        completion(
            None,
            vec![tool_call(
                "call_search",
                "tool_search",
                json!({"query":"select:demo__echo"}),
            )],
        ),
        completion(
            None,
            vec![tool_call(
                "call_mcp",
                "demo__echo",
                json!({"text":"MCP input marker"}),
            )],
        ),
        completion(Some("The MCP tool returned the requested marker."), vec![]),
    ]);
    let path = config(
        temp.path(),
        &endpoint.url,
        false,
        false,
        "mode = \"direct\"",
    );
    let mut config_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    write!(
        config_file,
        "\n[mcp]\nenabled = true\ndeferred_loading = true\n\n[[mcp.servers]]\nname = \"demo\"\ntransport = \"http\"\nurl = \"{}\"\n",
        mcp.url
    ).unwrap();
    drop(config_file);
    let output = run_cli(temp.path(), &path, None, Some("Use the local echo tool"));
    let requests = endpoint.finish();
    let mcp_requests = mcp.finish();
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("The MCP tool returned the requested marker."));
    assert!(tool_result(&requests[1], "call_search")
        .to_string()
        .contains("demo__echo"));
    assert_eq!(mcp_requests[3]["params"]["name"], "echo");
    assert_eq!(
        mcp_requests[3]["params"]["arguments"]["text"],
        "MCP input marker"
    );
    assert!(tool_result(&requests[2], "call_mcp")
        .to_string()
        .contains("MCP returned the requested marker"));
}

#[test]
fn cli_file_edits_reload_mcp_with_relative_or_absolute_config_paths() {
    for relative_cli_path in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let mcp = LocalMcp::start();
        let endpoint = ScriptedEndpoint::start(vec![
            completion(
                None,
                vec![tool_call(
                    "call_add_server",
                    "file_edit",
                    json!({
                        "path": if relative_cli_path { temp.path().join("assistant.toml").to_string_lossy().to_string() } else { "assistant.toml".to_string() },
                        "old_string": "[mcp]\nenabled = true\n",
                        "new_string": format!("[mcp]\nenabled = true\n\n[[mcp.servers]]\nname = \"demo\"\ntransport = \"http\"\nurl = \"{}\"\n", mcp.url)
                    }),
                )],
            ),
            completion(
                None,
                vec![tool_call(
                    "call_edit_again",
                    "file_edit",
                    json!({
                        "path": if relative_cli_path { temp.path().join("assistant.toml").to_string_lossy().to_string() } else { "assistant.toml".to_string() },
                        "old_string": "max_iterations = 8",
                        "new_string": "max_iterations = 9"
                    }),
                )],
            ),
            completion(
                None,
                vec![tool_call(
                    "call_search_reload",
                    "tool_search",
                    json!({"query":"select:demo__echo"}),
                )],
            ),
            completion(
                None,
                vec![tool_call(
                    "call_mcp_reload",
                    "demo__echo",
                    json!({"text":"MCP reload marker"}),
                )],
            ),
            completion(Some("The reloaded MCP tool responded."), vec![]),
        ]);
        let path = config(
            temp.path(),
            &endpoint.url,
            false,
            false,
            "mode = \"direct\"",
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n[mcp]\nenabled = true\ndeferred_loading = true\n")
            .unwrap();
        let cli_path = if relative_cli_path {
            Path::new("assistant.toml")
        } else {
            path.as_path()
        };
        let output = run_cli(
            temp.path(),
            cli_path,
            None,
            Some("Add and use the local MCP server"),
        );
        let requests = endpoint.finish();
        let mcp_requests = mcp.finish();
        assert_success(&output);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("The reloaded MCP tool responded.")
        );
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("max_iterations = 9"));
        assert_eq!(updated.matches("[[mcp.servers]]").count(), 1);
        assert!(tool_result(&requests[1], "call_add_server")
            .to_string()
            .contains("assistant.toml"));
        assert!(tool_result(&requests[2], "call_edit_again")
            .to_string()
            .contains("assistant.toml"));
        assert!(tool_result(&requests[3], "call_search_reload")
            .to_string()
            .contains("demo__echo"));
        assert!(tool_result(&requests[4], "call_mcp_reload")
            .to_string()
            .contains("MCP returned the requested marker"));
        assert_eq!(
            mcp_requests[3]["params"]["arguments"]["text"],
            "MCP reload marker"
        );
        let registered = requests[4]["tools"]
            .as_array()
            .expect("model tool list missing after reload");
        assert_eq!(
            registered
                .iter()
                .filter(|tool| tool["function"]["name"] == "demo__echo")
                .count(),
            1
        );
    }
}
