//! Explicit live acceptance for additional installed Agent CLIs.
//! Only the Host resolves the selected subscription account. Each CLI receives
//! a temporary virtual key and isolated configuration, state, and workspace.

#[path = "common/live_subscription.rs"]
mod live_subscription;

use base64::Engine as _;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use token_station_cli::{config::ClientConfig, gateway::Gateway, server};
use token_station_metrics::{Recorder, RequestRecord};

const PROMPT: &str = "Use your native file-reading tool to read marker.txt in the current working directory exactly once. Do not inspect any other file, directory, configuration, skill, memory, or network resource. Do not use shell commands. Then reply with LIVE_AGENT_TOOL_OK followed by the exact marker.txt contents. No other work is needed.";

const EDIT_PROMPT: &str = "Perform exactly this isolated fixture workflow. Use a native file-reading tool to read marker.txt. Use a native file-writing or editing tool to write edited.txt with the marker, then a newline, then EDITED_OK, then a final newline. Use a native shell tool to execute exactly /bin/cat edited.txt in the current working directory, without extra commands or shell operators. Do not use shell for reading marker.txt or writing files. Do not inspect any other file, directory, configuration, skill, memory, or network resource. If permission is denied, stop and report that boundary. Otherwise reply with LIVE_AGENT_TOOL_OK followed by the exact output of the shell verification. No other work is needed.";

#[derive(Default)]
struct Records(Mutex<Vec<RequestRecord>>);
impl Recorder for Records {
    fn record(&self, record: &RequestRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Every child receives a fresh process group. Cleanup is limited to that group.
struct IsolatedChild(std::process::Child);
impl Drop for IsolatedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", self.0.id())])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Live {
    scratch: Scratch,
    home: PathBuf,
    work: PathBuf,
    key: String,
    marker: String,
    url: String,
    records: Arc<Records>,
    runtime: Option<tokio::runtime::Runtime>,
    server: tokio::task::JoinHandle<()>,
}
impl Live {
    fn new(agent: &str) -> Self {
        let required = |name| {
            std::env::var(name)
                .unwrap_or_else(|_| panic!("Set {name} for explicit live acceptance"))
        };
        let data = required("TOKEN_STATION_CHATGPT_LIVE_DATA_DIR");
        let account = if live_subscription::use_codex() {
            String::new()
        } else {
            required("TOKEN_STATION_CHATGPT_LIVE_ACCOUNT_ID")
        };
        let model = required("TOKEN_STATION_CHATGPT_LIVE_MODEL");
        let random = || {
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes).unwrap();
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        };
        let scratch = Scratch(std::env::temp_dir().join(format!(
            "ts-plan-agents-{}-{}",
            std::process::id(),
            random()
        )));
        let home = scratch.0.join("home");
        let work = scratch.0.join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let key = random();
        let marker = format!("PLAN_AGENT_{}", random());
        std::fs::write(work.join("marker.txt"), &marker).unwrap();
        let mut model_config =
            json!({"model":model,"tool":true,"tool_state":"verified","context_window":200_000});
        if agent == "nous-hermes-agent" {
            model_config["json_schema"] = json!(true);
            model_config["json_schema_state"] = json!("declared");
        }
        let config: ClientConfig = serde_json::from_value(live_subscription::configure(json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":data,"metrics":false},
            "plugins":{"dir":Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins-dist"),"allow_unsigned":true,"agents":["agent-openai"],"providers":{"chatgpt-plan":"provider-chatgpt-plan-v2"}},
            "upstreams":{"plan":{"provider":"chatgpt-plan","base_url":"https://api.openai.com/v1","auth":{"slot":"chatgpt_access_token","chatgpt_account":account},"models":[model_config]}},
            "router":{"version":1,"pools":{"main":[{"upstream":"plan","model":model}]},"default_pool":"main"}
        }), "plan", Path::new(&data))).unwrap();
        config.validate().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let records = Arc::new(Records::default());
        let gateway =
            Gateway::new_with_provider_runtime(&config, records.clone(), runtime.handle().clone())
                .unwrap()
                .with_body_log(Arc::new(
                    token_station_cli::bodylog::BodyLog::open(&scratch.0).unwrap(),
                ));
        let state = server::AppState::new(
            Arc::new(gateway),
            Some(Arc::from(key.as_str())),
            Arc::new(token_station_cli::admin::AdminContext::from_config(&config).unwrap()),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://{}/agents/{agent}/v1",
            listener.local_addr().unwrap()
        );
        let server = runtime.spawn(async move {
            server::serve(state, tokio::net::TcpListener::from_std(listener).unwrap())
                .await
                .unwrap();
        });
        Self {
            scratch,
            home,
            work,
            key,
            marker,
            url,
            records,
            runtime: Some(runtime),
            server,
        }
    }

    fn write(&self, path: &Path, text: &str) {
        assert!(
            path.starts_with(&self.scratch.0),
            "Write only inside the isolated test root"
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn execute(
        &self,
        binary: &Path,
        args: &[&str],
        environment: &[(&str, String)],
    ) -> (std::process::ExitStatus, String, String) {
        let output_path = self.scratch.0.join("stdout.txt");
        let error_path = self.scratch.0.join("stderr.txt");
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("LANG", "en_US.UTF-8")
            .env("NO_COLOR", "1")
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("TMPDIR", &self.scratch.0)
            .current_dir(&self.work)
            .args(args)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&output_path).unwrap())
            .stderr(std::fs::File::create(&error_path).unwrap());
        for (key, value) in environment {
            command.env(key, value);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = IsolatedChild(
            command
                .spawn()
                .expect("The selected installed CLI must start"),
        );
        let deadline = Instant::now() + Duration::from_mins(3);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.0.kill();
                let _ = child.0.wait();
                panic!("The isolated CLI exceeded its three-minute deadline");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        drop(child);
        let output = std::fs::read_to_string(output_path).unwrap();
        let errors = std::fs::read_to_string(error_path).unwrap();
        (status, output, errors)
    }

    fn run(&self, binary: &Path, args: &[&str], environment: &[(&str, String)]) {
        let (status, output, errors) = self.execute(binary, args, environment);
        let records = self.records.0.lock().unwrap();
        for record in records.iter() {
            eprintln!(
                "agent receipt: status={} error={:?} subscription={} attempts={} method={:?} path={:?} protocol={}",
                record.status,
                record.error_code,
                record.subscription_billing,
                record.attempts,
                record.request_method,
                record.path_kind,
                record.protocol
            );
        }
        if !status.success()
            || records.is_empty()
            || records.iter().any(|record| {
                !record.subscription_billing && !self.hermes_auxiliary_boundary(record)
            })
            || !output.contains(&self.marker)
            || !output.contains("LIVE_AGENT_TOOL_OK")
        {
            // This subprocess has only a synthetic local key. Do not print
            // Host traces, OAuth material, or complete child environments.
            eprintln!(
                "CLI diagnostic: exit={status}, stderr={}, stdout={}",
                errors
                    .replace(&self.key, "[local-key]")
                    .chars()
                    .take(2500)
                    .collect::<String>(),
                output
                    .replace(&self.key, "[local-key]")
                    .chars()
                    .rev()
                    .take(2200)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            );
        }
        if records
            .iter()
            .any(|record| !record.subscription_billing && !self.hermes_auxiliary_boundary(record))
        {
            for entry in std::fs::read_dir(self.scratch.0.join("request-bodies")).unwrap() {
                let exchange: token_station_cli::bodylog::PlaintextExchange =
                    serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
                if let Some(response) = exchange.http_trace.agent_response
                    && let Ok(body) = serde_json::from_str::<Value>(&response.body)
                    && body.get("error").is_some()
                {
                    eprintln!(
                        "Gateway error diagnostic: {}",
                        body.to_string()
                            .replace(&self.key, "[local-key]")
                            .chars()
                            .take(1800)
                            .collect::<String>()
                    );
                }
            }
        }
        for record in records.iter() {
            assert!(record.subscription_billing || self.hermes_auxiliary_boundary(record));
            assert_eq!(record.cost_micros, None);
        }
        assert!(status.success(), "The installed CLI exited with an error");
        assert!(
            !records.is_empty(),
            "The CLI did not reach the isolated gateway"
        );
        assert!(
            output.contains("LIVE_AGENT_TOOL_OK") && output.contains(&self.marker),
            "The CLI final output must contain the exact random marker"
        );
        assert!(
            !output.contains(&self.key),
            "The CLI must not expose the local key"
        );
        drop(records);
        self.verify_tool_history();
    }

    fn verify_hermes_title(&self, require_success: bool) {
        let mut title_response = false;
        let mut reasoning_rejection = false;
        let mut first_format: Option<Value> = None;
        for entry in std::fs::read_dir(self.scratch.0.join("request-bodies")).unwrap() {
            let exchange: token_station_cli::bodylog::PlaintextExchange =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            let Some(request) = exchange.http_trace.agent_request else {
                continue;
            };
            let Ok(body) = serde_json::from_str::<Value>(&request.body) else {
                continue;
            };
            if body["response_format"]["json_schema"]["name"] != "session_title" {
                continue;
            }
            if let Some(format) = &first_format {
                assert_eq!(
                    &body["response_format"], format,
                    "Client recovery must preserve the full schema contract"
                );
            } else {
                first_format = Some(body["response_format"].clone());
            }
            let response = exchange.http_trace.agent_response.unwrap();
            if response.status == 499 && !require_success {
                assert_eq!(body["reasoning_effort"], "none");
                eprintln!(
                    "Hermes oneshot boundary: background session-title request was cancelled at exit"
                );
                reasoning_rejection = true;
                continue;
            }
            if response.status == 400 {
                assert_eq!(body["reasoning_effort"], "none");
                let error: Value = serde_json::from_str(&response.body).unwrap();
                assert_eq!(error["error"]["code"], "invalid_request");
                assert_eq!(
                    error["error"]["message"],
                    "Unsupported value for reasoning_effort. The selected model does not support this reasoning effort."
                );
                assert_eq!(exchange.http_trace.upstream_exchanges.len(), 1);
                let upstream = exchange.http_trace.upstream_exchanges[0]
                    .response
                    .as_ref()
                    .unwrap();
                let upstream_error = if upstream.status == 200 {
                    upstream_json_rows(&upstream.body)
                        .into_iter()
                        .find(|row| row["type"] == "error")
                        .expect("Codex must emit the upstream SSE error")
                } else {
                    assert_eq!(upstream.status, 400);
                    serde_json::from_str::<Value>(&upstream.body).unwrap()
                };
                assert_eq!(upstream_error["error"]["code"], "unsupported_value");
                assert_eq!(upstream_error["error"]["param"], "reasoning.effort");
                eprintln!(
                    "Hermes title boundary: unsupported reasoning disable; client recovery requires a live client"
                );
                reasoning_rejection = true;
                continue;
            }
            assert!(
                body.get("reasoning_effort").is_none() || body["reasoning_effort"] == "low",
                "Only the client reasoning recovery may change this field"
            );
            assert_eq!(
                response.status, 200,
                "Hermes session-title structured output must succeed"
            );
            let envelope: Value = serde_json::from_str(&response.body).unwrap();
            let title: Value = serde_json::from_str(
                envelope["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert!(
                title["title"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
            );
            title_response = true;
        }
        if require_success {
            assert!(
                reasoning_rejection,
                "The default Hermes title request must exercise safe reasoning recovery"
            );
            assert!(
                title_response,
                "Hermes must receive its structured session-title response"
            );
        } else {
            assert!(
                title_response || reasoning_rejection,
                "The oneshot title boundary must be observed explicitly"
            );
        }
    }

    fn hermes_auxiliary_boundary(&self, record: &RequestRecord) -> bool {
        if record.agent_id.as_deref() != Some("nous-hermes-agent") || record.attempts != 0 {
            return false;
        }
        for entry in std::fs::read_dir(self.scratch.0.join("request-bodies")).unwrap() {
            let exchange: token_station_cli::bodylog::PlaintextExchange =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            if exchange.request_id != record.request_id {
                continue;
            }
            assert!(exchange.http_trace.upstream_exchanges.is_empty());
            let request = exchange.http_trace.agent_request.unwrap();
            let response = exchange.http_trace.agent_response.unwrap();
            let error: Value = serde_json::from_str(&response.body).unwrap();
            if request.method == "GET"
                && request.url.ends_with("/v1/props")
                && response.status == 404
                && error["error"]["message"] == "no inbound adapter claims GET /v1/props"
            {
                eprintln!(
                    "Hermes auxiliary boundary: optional local-server props discovery is unsupported"
                );
                return true;
            }
        }
        false
    }

    #[allow(clippy::too_many_lines)] // One live transcript verifies correlated file and shell effects.
    fn verify_write_and_shell(&self) {
        let expected = format!("{}\nEDITED_OK\n", self.marker);
        assert_eq!(
            std::fs::read_to_string(self.work.join("edited.txt")).unwrap(),
            expected
        );
        let mut write_result = false;
        let mut shell_result = false;
        let mut final_answer = false;
        let mut diagnostics = Vec::new();
        for entry in std::fs::read_dir(self.scratch.0.join("request-bodies")).unwrap() {
            let exchange: token_station_cli::bodylog::PlaintextExchange =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            if let Some(response) = &exchange.http_trace.agent_response {
                let documents: Vec<Value> = if let Ok(value) = serde_json::from_str(&response.body)
                {
                    vec![value]
                } else {
                    response
                        .body
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:"))
                        .filter_map(|data| serde_json::from_str(data.trim()).ok())
                        .collect()
                };
                let text = documents
                    .iter()
                    .filter_map(|value| {
                        value["choices"][0]["delta"]["content"]
                            .as_str()
                            .or_else(|| value["choices"][0]["message"]["content"].as_str())
                    })
                    .collect::<String>();
                final_answer |= text.contains("LIVE_AGENT_TOOL_OK")
                    && text.contains(&self.marker)
                    && text.contains("EDITED_OK");
            }
            let Some(request) = exchange.http_trace.agent_request else {
                continue;
            };
            if request.method != "POST" {
                continue;
            }
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            for result in messages.iter().filter(|value| value["role"] == "tool") {
                let call = messages
                    .iter()
                    .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten())
                    .find(|call| call["id"] == result["tool_call_id"])
                    .expect("Every tool result must reference an earlier native call");
                let name = call["function"]["name"]
                    .as_str()
                    .unwrap()
                    .to_ascii_lowercase();
                let arguments = call["function"]["arguments"].as_str().unwrap();
                diagnostics.push(format!(
                    "name={name}, arguments={}, result={}",
                    arguments
                        .replace(&self.key, "[local-key]")
                        .chars()
                        .take(220)
                        .collect::<String>(),
                    result["content"]
                        .to_string()
                        .replace(&self.key, "[local-key]")
                        .chars()
                        .take(700)
                        .collect::<String>()
                ));
                if (name.contains("write") || name.contains("edit"))
                    && arguments.contains("edited.txt")
                {
                    write_result = true;
                }
                if name.contains("bash")
                    || name.contains("shell")
                    || name == "exec"
                    || name == "run_terminal_command"
                    || name == "terminal"
                {
                    let args: Value = serde_json::from_str(arguments).unwrap();
                    let command = args["command"]
                        .as_str()
                        .or_else(|| args["cmd"].as_str())
                        .expect("The shell tool must declare its command");
                    assert_eq!(
                        command.trim(),
                        "/bin/cat edited.txt",
                        "Only the exact fixture verification command is allowed"
                    );
                    shell_result |= result["content"].to_string().contains(&self.marker)
                        && result["content"].to_string().contains("EDITED_OK");
                }
            }
        }
        assert!(
            write_result,
            "The model must receive a native write/edit result"
        );
        if !shell_result {
            for diagnostic in diagnostics {
                eprintln!("fixture diagnostic: {diagnostic}");
            }
        }
        assert!(
            shell_result,
            "The model must receive the native shell verification output"
        );
        assert!(
            final_answer,
            "The final assistant response must include the shell verification result"
        );
    }

    fn verify_tool_history(&self) {
        let mut found = false;
        let mut final_answer = false;
        for entry in std::fs::read_dir(self.scratch.0.join("request-bodies")).unwrap() {
            let exchange: token_station_cli::bodylog::PlaintextExchange =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            if let Some(response) = &exchange.http_trace.agent_response {
                let documents: Vec<Value> =
                    if let Ok(document) = serde_json::from_str::<Value>(&response.body) {
                        vec![document]
                    } else {
                        response
                            .body
                            .lines()
                            .filter_map(|line| line.strip_prefix("data:"))
                            .filter_map(|data| serde_json::from_str(data.trim()).ok())
                            .collect()
                    };
                let text = documents
                    .iter()
                    .filter_map(|v| {
                        v["choices"][0]["delta"]["content"]
                            .as_str()
                            .or_else(|| v["choices"][0]["message"]["content"].as_str())
                    })
                    .collect::<String>();
                final_answer |= text.contains("LIVE_AGENT_TOOL_OK") && text.contains(&self.marker);
            }
            let Some(request) = exchange.http_trace.agent_request else {
                continue;
            };
            if request.method != "POST" {
                continue;
            }
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let messages = body["messages"]
                .as_array()
                .expect("The connector must use Chat Completions");
            for result in messages
                .iter()
                .filter(|v| v["role"] == "tool" && v["content"].to_string().contains(&self.marker))
            {
                let id = &result["tool_call_id"];
                let call = messages
                    .iter()
                    .flat_map(|v| v["tool_calls"].as_array().into_iter().flatten())
                    .find(|v| &v["id"] == id)
                    .expect("A native tool result must match its call ID");
                let name = call["function"]["name"]
                    .as_str()
                    .unwrap()
                    .to_ascii_lowercase();
                if !name.contains("read") {
                    continue;
                }
                assert!(
                    call["function"]["arguments"]
                        .to_string()
                        .contains("marker.txt")
                );
                found = true;
            }
        }
        assert!(
            found,
            "The model must receive the native read result with the random marker"
        );
        assert!(
            final_answer,
            "The final assistant response must contain the marker after the tool result"
        );
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.server.abort();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

fn binary(name: &str) -> PathBuf {
    let variable = format!("TOKEN_STATION_{name}_CLI");
    PathBuf::from(
        std::env::var(&variable).unwrap_or_else(|_| panic!("Set {variable} to the installed CLI")),
    )
}

#[test]
#[ignore = "Consumes the explicitly selected ChatGPT plan through an isolated installed Grok CLI"]
fn live_grok_reads_a_random_marker() {
    grok_fixture(false);
}

#[test]
#[ignore = "Consumes the selected ChatGPT plan for isolated read, write, and shell verification"]
fn live_grok_writes_and_verifies_a_random_marker() {
    grok_fixture(true);
}

fn grok_fixture(deep: bool) {
    let live = Live::new("grok-build");
    let grok_home = live.home.join(".grok");
    let mut config = "[models]\ndefault = \"tokenstation\"\n".to_owned();
    for id in ["tokenstation", "grok-4.6", "grok-4.5"] {
        write!(config, "\n[model.\"{id}\"]\nmodel = \"auto\"\napi_backend = \"chat_completions\"\nbase_url = {}\napi_key = {}\ncontext_window = 200000\nmax_completion_tokens = 8192\n",json!(live.url),json!(live.key)).unwrap();
    }
    live.write(&grok_home.join("config.toml"), &config);
    live.run(
        &binary("GROK"),
        &[
            "--single",
            if deep { EDIT_PROMPT } else { PROMPT },
            "--model",
            "tokenstation",
            "--output-format",
            "streaming-messages-json",
            "--tools",
            if deep { "Read,Write,Edit,Bash" } else { "Read" },
            "--allow",
            "Read(./marker.txt)",
            "--allow",
            "Write(./edited.txt)",
            "--allow",
            "Edit(./edited.txt)",
            "--allow",
            "Bash(/bin/cat edited.txt)",
            "--max-turns",
            if deep { "6" } else { "3" },
            "--no-subagents",
            "--disable-web-search",
            "--leader-socket",
            live.scratch.0.join("leader.sock").to_str().unwrap(),
        ],
        &[("GROK_HOME", grok_home.display().to_string())],
    );
    if deep {
        live.verify_write_and_shell();
    }
}

#[test]
#[ignore = "Consumes the explicitly selected ChatGPT plan through an isolated installed Kimi CLI"]
fn live_kimi_reads_a_random_marker() {
    kimi_fixture(false);
}

#[test]
#[ignore = "Consumes the selected ChatGPT plan for isolated read, write, and shell verification"]
fn live_kimi_writes_and_verifies_a_random_marker() {
    kimi_fixture(true);
}

fn kimi_fixture(deep: bool) {
    let live = Live::new("kimi-code");
    let kimi_home = live.home.join(".kimi-code");
    live.write(&kimi_home.join("config.toml"),&format!("default_model = \"tokenstation-auto\"\n[providers.tokenstation]\ntype = \"openai\"\nbase_url = {}\napi_key = {}\n[models.tokenstation-auto]\nprovider = \"tokenstation\"\nmodel = \"auto\"\nmax_context_size = 200000\nmax_output_size = 8192\n",json!(live.url),json!(live.key)));
    if deep {
        let path = kimi_home.join("config.toml");
        let mut config = std::fs::read_to_string(&path).unwrap();
        for pattern in [
            format!("Read({})", live.work.join("marker.txt").display()),
            format!("Write({})", live.work.join("edited.txt").display()),
            format!("Edit({})", live.work.join("edited.txt").display()),
            "Bash(/bin/cat edited.txt)".into(),
        ] {
            write!(
                config,
                "\n[[permission.rules]]\ndecision = \"allow\"\npattern = {}\n",
                json!(pattern)
            )
            .unwrap();
        }
        config.push_str("\n[loop_control]\nmax_steps_per_turn = 6\nmax_retries_per_step = 0\n");
        live.write(&path, &config);
    }
    let skills = live.home.join("empty-skills");
    std::fs::create_dir_all(&skills).unwrap();
    live.run(
        &binary("KIMI"),
        &[
            "--prompt",
            if deep { EDIT_PROMPT } else { PROMPT },
            "--model",
            "tokenstation-auto",
            "--output-format",
            "stream-json",
            "--skills-dir",
            skills.to_str().unwrap(),
        ],
        &[("KIMI_CODE_HOME", kimi_home.display().to_string())],
    );
    if deep {
        live.verify_write_and_shell();
    }
}

#[test]
#[ignore = "Consumes the explicitly selected ChatGPT plan through an isolated installed Hermes CLI"]
fn live_hermes_reads_a_random_marker() {
    hermes_fixture(false);
}

#[test]
#[ignore = "Consumes the selected ChatGPT plan for isolated read, write, and shell verification"]
fn live_hermes_writes_and_verifies_a_random_marker() {
    hermes_fixture(true);
}

fn hermes_fixture(deep: bool) {
    let live = Live::new("nous-hermes-agent");
    let hermes_home = live.home.join(".hermes");
    let mut config = json!({"model":{"default":"auto","provider":"custom","base_url":live.url,"api_key":live.key,"api_mode":"chat_completions","context_length":200_000},"agent":{"max_turns":if deep { 6 } else { 3 }},"security":{"allow_lazy_installs":false}});
    let guard = live.scratch.0.join("hermes-fixture-guard.py");
    live.write(&guard, r"import json, pathlib, sys
payload = json.load(sys.stdin)
root = pathlib.Path.cwd().resolve()
args = payload.get('tool_input') or {}
name = payload.get('tool_name')
target = (root / args.get('path', '')).resolve()
allowed = (name == 'read_file' and target in [root / 'marker.txt', root / 'edited.txt']) or (name == 'write_file' and target == root / 'edited.txt') or (name == 'terminal' and args.get('command') == '/bin/cat edited.txt' and not args.get('background') and not args.get('pty') and pathlib.Path(args.get('workdir') or root).resolve() == root)
if allowed:
    with open(root.parent / 'hermes-guard-audit.txt', 'a') as audit:
        audit.write(name + '\n')
    print('{}')
else:
    print(json.dumps({'action':'block', 'message':'Only exact fixture files and verification command are authorized'}))");
    let guard_python =
        std::env::var("TOKEN_STATION_HERMES_PYTHON").unwrap_or_else(|_| "/usr/bin/python3".into());
    let guard_command = format!("{} -I -B {}", json!(guard_python), json!(guard));
    config["hooks"] =
        json!({"pre_tool_call":[{"command":guard_command,"timeout":5,"fail_closed":true}]});
    config["command_allowlist"] = json!(["/bin/cat edited.txt"]);
    live.write(
        &hermes_home.join("shell-hooks-allowlist.json"),
        &json!({"approvals":[{"event":"pre_tool_call","command":guard_command}]}).to_string(),
    );
    live.write(&hermes_home.join("config.yaml"), &config.to_string());
    // Explicitly reuse installed code and an existing dependency environment.
    // Isolated mode disables Python environment imports and bytecode writes.
    // Join the official background title worker before the test process exits.
    let source = std::env::var("TOKEN_STATION_HERMES_SOURCE").ok();
    let python = std::env::var_os("TOKEN_STATION_HERMES_PYTHON");
    assert_eq!(
        source.is_some(),
        python.is_some(),
        "Set both Hermes runtime overrides"
    );
    let executable = python.map_or_else(|| binary("HERMES"), PathBuf::from);
    let mut args = Vec::new();
    if let Some(source) = source.as_deref() {
        args.extend([
            "-I", "-B", "-c",
            "import sys\nsys.path.insert(0,sys.argv.pop(1))\nfrom hermes_cli.main import main\ntry:\n    result=main()\nfinally:\n    from agent.title_generator import wait_for_title_upgrades\n    wait_for_title_upgrades(45)\nsys.exit(result)",
            source,
        ]);
    }
    args.extend([
        "chat",
        "--query",
        if deep { EDIT_PROMPT } else { PROMPT },
        "--oneshot",
        "--format",
        "stream-json",
        "--toolsets",
        if deep { "file,terminal" } else { "file" },
        "--ignore-rules",
        "--max-turns",
        if deep { "6" } else { "3" },
        "--run-budget",
        "120",
        "--provider",
        "custom",
        "--model",
        "auto",
    ]);
    live.run(
        &executable,
        &args,
        &[
            ("HERMES_HOME", hermes_home.display().to_string()),
            ("HERMES_DISABLE_LAZY_INSTALLS", "1".into()),
            ("UV_OFFLINE", "1".into()),
            ("PIP_NO_INDEX", "1".into()),
            ("PYTHONDONTWRITEBYTECODE", "1".into()),
        ],
    );
    let audit = std::fs::read_to_string(live.work.parent().unwrap().join("hermes-guard-audit.txt"))
        .unwrap();
    assert!(audit.lines().any(|name| name == "read_file"));
    if deep {
        assert!(audit.lines().any(|name| name == "write_file"));
        assert!(audit.lines().any(|name| name == "terminal"));
        live.verify_write_and_shell();
    }
    live.verify_hermes_title(false);
}

#[test]
#[ignore = "Consumes the explicitly selected ChatGPT plan through an isolated installed OpenClaw CLI"]
fn live_openclaw_reads_a_random_marker() {
    openclaw_fixture(false);
}

#[test]
#[ignore = "Consumes the selected ChatGPT plan for isolated read, write, and shell verification"]
fn live_openclaw_writes_and_verifies_a_random_marker() {
    openclaw_fixture(true);
}

fn openclaw_fixture(deep: bool) {
    let live = Live::new("openclaw");
    let state = live.home.join(".openclaw");
    let config = state.join("openclaw.json");
    live.write(&config,&json!({
        "models":{"providers":{"tokenstation":{"baseUrl":live.url,"apiKey":live.key,"auth":"api-key","api":"openai-completions","models":[{"id":"auto","name":"Token Station Auto","reasoning":false,"input":["text"],"contextWindow":200_000,"maxTokens":8192}]}}},
        "agents":{"defaults":{"model":{"primary":"tokenstation/auto"},"workspace":live.work,"skipBootstrap":true}},
        "tools":{"allow":if deep {vec!["read","write","edit","exec"]} else {vec!["read"]},"fs":{"workspaceOnly":true},"exec":{"host":"gateway","mode":"allowlist","safeBins":[]}},"skills":{"allowBundled":["tokenstation-no-skills"]},"plugins":{"enabled":false}
    }).to_string());
    if deep {
        live.write(&live.scratch.0.join("exec-approvals-input.json"), &json!({"version":1,"defaults":{"security":"allowlist","ask":"on-miss","askFallback":"deny","autoAllowSkills":false},"agents":{"main":{"allowlist":[{"pattern":"/bin/cat","argPattern":"^edited\\.txt$"}]}}}).to_string());
    }
    if deep {
        let (status, _, errors) = live.execute(
            &binary("OPENCLAW"),
            &[
                "approvals",
                "set",
                "--file",
                live.scratch
                    .0
                    .join("exec-approvals-input.json")
                    .to_str()
                    .unwrap(),
            ],
            &[
                ("OPENCLAW_STATE_DIR", state.display().to_string()),
                ("OPENCLAW_CONFIG_PATH", config.display().to_string()),
            ],
        );
        assert!(
            status.success(),
            "The isolated command allowlist must load: {}",
            errors.replace(&live.key, "[local-key]")
        );
    }
    live.run(
        &binary("OPENCLAW"),
        &[
            "agent",
            "--local",
            "--session-id",
            "tokenstation-live-acceptance",
            "--model",
            "tokenstation/auto",
            "--message",
            if deep { EDIT_PROMPT } else { PROMPT },
            "--json",
            "--timeout",
            "120",
        ],
        &[
            ("OPENCLAW_STATE_DIR", state.display().to_string()),
            ("OPENCLAW_CONFIG_PATH", config.display().to_string()),
        ],
    );
    if deep {
        live.verify_write_and_shell();
    }
}

#[test]
#[ignore = "Consumes the explicitly selected ChatGPT plan through the isolated WorkBuddy bundled CLI"]
fn live_workbuddy_reads_a_random_marker() {
    workbuddy_fixture(false);
}

#[test]
#[ignore = "Consumes the selected ChatGPT plan for isolated read, write, and shell verification"]
fn live_workbuddy_writes_and_verifies_a_random_marker() {
    workbuddy_fixture(true);
}

fn workbuddy_fixture(deep: bool) {
    let live = Live::new("workbuddy");
    let config = live.home.join(".workbuddy");
    live.write(&config.join("models.json"),&json!({"models":[{"id":"tokenstation-auto","name":"Token Station Auto","vendor":"Token Station","url":format!("{}/chat/completions",live.url),"apiKey":live.key,"supportsToolCall":true,"supportsImages":false,"supportsReasoning":false,"onlyReasoning":false,"useCustomProtocol":false,"maxInputTokens":200_000,"maxOutputTokens":8192}],"availableModels":["tokenstation-auto"]}).to_string());
    let canonical_work = std::fs::canonicalize(&live.work).unwrap();
    let read_rule = format!("Read({})", canonical_work.join("marker.txt").display());
    let write_rule = format!("Write({})", canonical_work.join("edited.txt").display());
    let edit_rule = format!("Edit({})", canonical_work.join("edited.txt").display());
    live.run(
        &binary("WORKBUDDY"),
        &[
            "--print",
            if deep { EDIT_PROMPT } else { PROMPT },
            "--model",
            "tokenstation-auto",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--setting-sources",
            "user",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--tools",
            if deep { "Read,Write,Edit,Bash" } else { "Read" },
            "--allowedTools",
            &read_rule,
            &write_rule,
            &edit_rule,
            "Bash(/bin/cat edited.txt)",
            "--max-turns",
            if deep { "6" } else { "3" },
        ],
        &[
            ("CODEBUDDY_CONFIG_DIR", config.display().to_string()),
            ("WORKBUDDY_CONFIG_DIR", config.display().to_string()),
            ("DISABLE_TELEMETRY", "1".into()),
        ],
    );
    if deep {
        live.verify_write_and_shell();
    }
}

#[test]
#[ignore = "Consumes the explicitly selected ChatGPT plan through an isolated installed DeepSeek Harness CLI"]
fn live_dsh_reads_a_random_marker() {
    dsh_fixture(false);
}

#[test]
#[ignore = "Consumes the selected ChatGPT plan for isolated read, write, and shell verification"]
fn live_dsh_writes_and_verifies_a_random_marker() {
    dsh_fixture(true);
}

fn dsh_fixture(deep: bool) {
    let live = Live::new("deepseek-harness");
    let home = live.home.join(".dsh");
    // DSH 0.2 profiles read Cordis patches, not the former settings.yaml.
    let mut patch = vec![
        json!({"id":"agent-default-model","config":{"provider":"tokenstation","model":"auto"}}),
        json!({"id":"llm-pi-ai","config":{"providers":{"tokenstation":{"displayName":"Token Station","apiKeyEnv":"TOKENSTATION_API_KEY","api":"openai-completions","baseURL":live.url,"retryPolicy":{"mode":"normal","maxRetries":0},"models":[{"id":"auto","name":"Token Station Auto","contextWindow":200_000,"maxTokens":8192}]}}}}),
    ];
    for id in [
        "tool-bash",
        "tool-pwsh",
        "tool-jobs",
        "tool-fs-search",
        "agent-instructions",
        "skill-filesystem",
        "tool-skill",
        "tool-subagent-control",
        "tool-subagent-list-agents",
        "tool-subagent",
        "tool-subagent-fork",
        "tool-workflow",
        "tool-todo",
        "tool-goal",
        "tool-ralph",
        "tool-web",
        "mcp-resources",
        "session-title-llm",
    ] {
        if deep && id == "tool-bash" {
            continue;
        }
        patch.push(json!({"id":id,"disabled":true}));
    }
    if deep {
        // This extra denial-only guard uses the installed Cordis extension API.
        // It cannot override the CLI sandbox or approve an escalation.
        let guard = live.scratch.0.join("fixture-guard.mjs");
        let source = r"import path from 'node:path';
export const inject = ['tools'];
export function apply(ctx) {
  const root = process.cwd();
  ctx.tools.guard(exec => {
    const args = exec.arguments ?? {};
    if (args.sandbox_permissions === 'require_escalated') return 'Fixture escalation is not authorized';
    if (exec.name === 'bash') {
      return args.command === '/bin/cat edited.txt' && !args.run_in_background && (!args.workdir || path.resolve(args.workdir) === root) ? undefined : 'Only the fixture verification command is authorized';
    }
    const target = typeof args.file_path === 'string' ? path.resolve(root, args.file_path) : '';
    if (exec.name === 'read' && [path.join(root,'marker.txt'),path.join(root,'edited.txt')].includes(target)) return;
    if (['write','edit'].includes(exec.name) && target === path.join(root,'edited.txt')) return;
    return 'Only the named fixture files are authorized';
  });
}
";
        live.write(&guard, source);
        patch.push(json!({"insert":[{"id":"fixture-guard","name":guard}]}));
    }
    live.write(&home.join("cordis.patch.yml"), &json!(patch).to_string());
    let credentials = home.join(".credentials.yaml");
    live.write(
        &credentials,
        &json!({"TOKENSTATION_API_KEY":live.key}).to_string(),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    live.run(
        &binary("DSH"),
        &[
            "--profile",
            "headless",
            if deep { EDIT_PROMPT } else { PROMPT },
            "--json",
        ],
        &[("DSH_HOME", home.display().to_string())],
    );
    if deep {
        live.verify_write_and_shell();
    }
}

fn upstream_json_rows(body: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(body).map_or_else(
        |_| {
            body.lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .and_then(|data| serde_json::from_str::<Value>(data.trim()).ok())
                })
                .collect()
        },
        |row| vec![row],
    )
}

fn print_safe_upstream_error(status: u16, body: &str) {
    let safe = |value: &Value| {
        let text = value.as_str().unwrap_or_default();
        if ["Bearer ", "sk-", "eyJ", "access_token", "refresh_token"]
            .iter()
            .any(|marker| text.contains(marker))
        {
            "[credential-like value omitted]".to_owned()
        } else {
            text.chars().take(400).collect::<String>()
        }
    };
    for row in upstream_json_rows(body) {
        let error = row.get("error").or_else(|| {
            row.get("response")
                .and_then(|response| response.get("error"))
        });
        if error.is_none() && row.get("detail").is_none() {
            continue;
        }
        let error = error.unwrap_or(&Value::Null);
        eprintln!(
            "Upstream structured error: {}",
            json!({"status":status,"event":safe(&row["type"]),"code":safe(&error["code"]),"param":safe(&error["param"]),"message":safe(&error["message"]),"detail":safe(&row["detail"])})
        );
    }
}

#[test]
#[ignore = "Consumes the selected ChatGPT plan for one isolated structured-output probe"]
fn live_chatgpt_plan_accepts_a_structured_title() {
    let live = Live::new("nous-hermes-agent");
    let request = live.scratch.0.join("title-request.json");
    let mut body = json!({
        "model":"auto", "stream":false,
        "messages":[{"role":"user","content":"Return JSON containing a short title for an isolated file verification task."}],
        "response_format":{"type":"json_schema","json_schema":{"name":"session_title","strict":true,"schema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"],"additionalProperties":false}}}
    });
    if let Ok(effort) = std::env::var("TOKEN_STATION_CHATGPT_PROBE_REASONING_EFFORT") {
        body["reasoning_effort"] = json!(effort);
    }
    live.write(&request, &body.to_string());
    let config = live.scratch.0.join("curl.cfg");
    live.write(
        &config,
        &format!(
            "url = {}\nheader = {}\nheader = \"Content-Type: application/json\"\n",
            json!(format!("{}/chat/completions", live.url)),
            json!(format!("Authorization: Bearer {}", live.key))
        ),
    );
    let (status, output, errors) = live.execute(
        Path::new("/usr/bin/curl"),
        &[
            "--silent",
            "--show-error",
            "--max-time",
            "90",
            "--config",
            config.to_str().unwrap(),
            "--data-binary",
            &format!("@{}", request.display()),
        ],
        &[],
    );
    assert!(
        status.success(),
        "Probe transport failed: {}",
        errors.replace(&live.key, "[local-key]")
    );
    let response: Value = serde_json::from_str(&output).unwrap();
    if response.get("error").is_some() {
        eprintln!(
            "Structured title probe: {}",
            response.to_string().replace(&live.key, "[local-key]")
        );
    }
    if response.get("error").is_some() {
        for entry in std::fs::read_dir(live.scratch.0.join("request-bodies")).unwrap() {
            let exchange: token_station_cli::bodylog::PlaintextExchange =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            for upstream in exchange.http_trace.upstream_exchanges {
                if let Some(response) = upstream.response {
                    print_safe_upstream_error(response.status, &response.body);
                }
            }
        }
    }
    let records = live.records.0.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].subscription_billing);
    assert_eq!(records[0].status, 200);
    assert_eq!(records[0].cost_micros, None);
    let title: Value = serde_json::from_str(
        response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(title["title"].as_str().is_some_and(|text| !text.is_empty()));
}

#[test]
#[ignore = "Consumes the selected plan for the installed Hermes title generator and its native reasoning recovery"]
fn live_hermes_title_recovers_without_changing_its_schema() {
    let live = Live::new("nous-hermes-agent");
    let python = PathBuf::from(
        std::env::var_os("TOKEN_STATION_HERMES_PYTHON")
            .expect("Set the installed Hermes Python environment"),
    );
    let source = std::env::var("TOKEN_STATION_HERMES_SOURCE")
        .expect("Set the installed Hermes source directory");
    let hermes_home = live.home.join(".hermes");
    let config = hermes_home.join("config.yaml");
    live.write(&config, &json!({"model":{"provider":"custom","default":"auto","base_url":live.url,"api_key":live.key,"api_mode":"chat_completions"},"security":{"allow_lazy_installs":false}}).to_string());
    let code = "import sys,json\nsys.path.insert(0,sys.argv[1])\nfrom agent.title_generator import generate_title\nruntime=json.load(open(sys.argv[2]))['model']\nruntime['model']=runtime.pop('default')\nprint(json.dumps({'title':generate_title('Verify an isolated file fixture',main_runtime=runtime)}))";
    let (status, output, errors) = live.execute(
        &python,
        &["-I", "-B", "-c", code, &source, config.to_str().unwrap()],
        &[
            ("HERMES_HOME", hermes_home.display().to_string()),
            ("HERMES_DISABLE_LAZY_INSTALLS", "1".into()),
            ("UV_OFFLINE", "1".into()),
            ("PIP_NO_INDEX", "1".into()),
        ],
    );
    assert!(
        status.success(),
        "Installed title generator failed: {}",
        errors.replace(&live.key, "[local-key]")
    );
    let title: Value = serde_json::from_str(output.trim()).unwrap();
    assert!(title["title"].as_str().is_some_and(|text| !text.is_empty()));
    let records = live.records.0.lock().unwrap();
    assert_eq!(
        records.iter().map(|r| r.status).collect::<Vec<_>>(),
        vec![400, 200]
    );
    for record in records.iter() {
        assert!(record.subscription_billing);
        assert_eq!(record.attempts, 1);
        assert_eq!(record.cost_micros, None);
    }
    drop(records);
    live.verify_hermes_title(true);
}
