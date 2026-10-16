use super::{auth, catalog, pipe, tool_error, MAX_REQUEST};
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Backend = Arc<Mutex<Option<pipe::Client>>>;
type Requests = Arc<Mutex<std::collections::BTreeMap<String, Arc<AtomicBool>>>>;

struct Bridge {
    backend: Backend,
    stop: Arc<AtomicBool>,
    heartbeat: Option<std::thread::JoinHandle<()>>,
}

impl Bridge {
    fn new() -> Self {
        let backend: Backend = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_backend = backend.clone();
        let worker_stop = stop.clone();
        let heartbeat = std::thread::spawn(move || {
            let mut ticks = 0;
            while !worker_stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(50));
                ticks += 1;
                if ticks < 100 {
                    continue;
                }
                ticks = 0;
                // An active request also proves liveness. Never queue a heartbeat
                // behind it or delay stdin EOF cleanup waiting for the mutex.
                if let Ok(mut client) = worker_backend.try_lock() {
                    if client
                        .as_mut()
                        .is_some_and(|client| client.call("__heartbeat", json!({})).is_err())
                    {
                        *client = None;
                    }
                }
            }
        });
        Self {
            backend,
            stop,
            heartbeat: Some(heartbeat),
        }
    }

    fn call(
        &self,
        client_name: &str,
        name: &str,
        arguments: Value,
        cancelled: Arc<AtomicBool>,
    ) -> Value {
        let result = (|| -> Result<Value, String> {
            if cancelled.load(Ordering::Acquire) {
                return Err("MCP_REQUEST_CANCELLED".into());
            }
            let mut client = self.backend.lock().map_err(|_| "MCP_BACKEND_UNAVAILABLE")?;
            if client.is_none() {
                let credential = auth::load_or_create(&auth::app_data()?)?;
                let mut connected =
                    pipe::Client::connect(&credential, client_name, cancelled.clone())?;
                connected.set_stop(self.stop.clone());
                *client = Some(connected);
            }
            let result = client
                .as_mut()
                .unwrap()
                .call_cancellable(name, arguments, &cancelled);
            if result.is_err() || cancelled.load(Ordering::Acquire) {
                *client = None;
            }
            result
        })();
        result.unwrap_or_else(|error| tool_error(&error))
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        if let Ok(mut client) = self.backend.lock() {
            *client = None;
        }
    }
}

pub fn run() -> Result<(), String> {
    let stdout = std::io::stdout();
    let bridge = Bridge::new();
    let requests: Requests = Arc::default();
    let reader = input(requests.clone())?;
    session_with_requests(
        reader,
        stdout.lock(),
        &requests,
        |client, name, arguments, cancelled| bridge.call(client, name, arguments, cancelled),
    )
}

/// The sole stdin reader keeps cancellation and EOF responsive while a tool is
/// in flight. At most 32 requests/frames are queued; cancelled queued tools never
/// reach the desktop. Cancelling a mutation closes its connection: its outcome
/// may be unknown, so a caller must inspect state rather than automatically retry.
fn input(requests: Requests) -> Result<QueuedInput, String> {
    let (send, receive) = std::sync::mpsc::sync_channel(32);
    std::thread::Builder::new()
        .name("arkdock-mcp-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            pump(stdin.lock(), send, &requests);
        })
        .map_err(|_| "MCP_STDIN_WORKER_FAILED")?;
    Ok(QueuedInput {
        receive,
        current: std::io::Cursor::new(Vec::new()),
        ended: false,
    })
}

fn pump(
    mut reader: impl BufRead,
    send: std::sync::mpsc::SyncSender<Result<Vec<u8>, String>>,
    requests: &Requests,
) {
    loop {
        let bytes = match line(&mut reader) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => break,
            Err(error) => {
                let _ = send.try_send(Err(error));
                break;
            }
        };
        if let Ok(request) = serde_json::from_slice::<Value>(&bytes) {
            if request["jsonrpc"] == "2.0"
                && request.get("id").is_none()
                && request["method"] == "notifications/cancelled"
            {
                if let Ok(pending) = requests.lock() {
                    if let Some(cancelled) =
                        pending.get(&request["params"]["requestId"].to_string())
                    {
                        cancelled.store(true, Ordering::Release);
                    }
                }
                continue;
            }
            if let Some(id) = request.get("id") {
                let Ok(mut pending) = requests.lock() else {
                    break;
                };
                if pending.len() >= 32 || pending.contains_key(&id.to_string()) {
                    break;
                }
                pending.insert(id.to_string(), Arc::new(AtomicBool::new(false)));
            }
        }
        if send.try_send(Ok(bytes)).is_err() {
            break;
        }
    }
    if let Ok(pending) = requests.lock() {
        for cancelled in pending.values() {
            cancelled.store(true, Ordering::Release);
        }
    }
}

struct QueuedInput {
    receive: std::sync::mpsc::Receiver<Result<Vec<u8>, String>>,
    current: std::io::Cursor<Vec<u8>>,
    ended: bool,
}
impl Read for QueuedInput {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let bytes = self.fill_buf()?;
        let length = bytes.len().min(output.len());
        output[..length].copy_from_slice(&bytes[..length]);
        self.consume(length);
        Ok(length)
    }
}
impl BufRead for QueuedInput {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.current.position() as usize == self.current.get_ref().len() && !self.ended {
            match self.receive.recv() {
                Ok(Ok(bytes)) => self.current = std::io::Cursor::new(bytes),
                Ok(Err(error)) => return Err(std::io::Error::other(error)),
                Err(_) => self.ended = true,
            }
        }
        self.current.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.current.consume(amount);
    }
}

/// Read at most one bounded MCP JSON line. fill_buf avoids allocating an
/// attacker-controlled unterminated line as BufRead::read_line would do.
fn line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut result = Vec::new();
    loop {
        let chunk = reader.fill_buf().map_err(|_| "MCP_STDIN_FAILED")?;
        if chunk.is_empty() {
            return if result.is_empty() {
                Ok(None)
            } else {
                Err("MCP_FRAME_TRUNCATED".into())
            };
        }
        let length = chunk
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|end| end + 1)
            .unwrap_or(chunk.len());
        if result.len() + length > MAX_REQUEST {
            return Err("MCP_FRAME_TOO_LARGE".into());
        }
        let complete = chunk[length - 1] == b'\n';
        result.extend_from_slice(&chunk[..length]);
        reader.consume(length);
        if complete {
            return Ok(Some(result));
        }
    }
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

#[cfg(test)]
fn session(
    reader: impl BufRead,
    writer: impl Write,
    mut call: impl FnMut(&str, &str, Value) -> Value,
) -> Result<(), String> {
    session_with_requests(reader, writer, &Arc::default(), |client, name, args, _| {
        call(client, name, args)
    })
}

fn session_with_requests(
    mut reader: impl BufRead,
    mut writer: impl Write,
    requests: &Requests,
    mut call: impl FnMut(&str, &str, Value, Arc<AtomicBool>) -> Value,
) -> Result<(), String> {
    let mut initialized = false;
    let mut ready = false;
    let mut client_name = String::new();
    while let Some(bytes) = line(&mut reader)? {
        let request: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                write(&mut writer, &error(Value::Null, -32700, "Parse error"))?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        let valid_id = id
            .as_ref()
            .is_none_or(|id| id.is_string() || id.is_i64() || id.is_u64());
        if !request.is_object()
            || request["jsonrpc"] != "2.0"
            || !request["method"].is_string()
            || !valid_id
            || request
                .get("params")
                .is_some_and(|params| !params.is_object())
        {
            write(&mut writer, &error(Value::Null, -32600, "Invalid Request"))?;
            if let (Some(id), Ok(mut pending)) = (id, requests.lock()) {
                pending.remove(&id.to_string());
            }
            continue;
        }
        let method = request["method"].as_str().unwrap();
        if id.is_none() {
            if method == "notifications/initialized" && initialized {
                ready = true;
            }
            continue;
        }
        let id = id.unwrap();
        let cancelled = requests
            .lock()
            .ok()
            .and_then(|pending| pending.get(&id.to_string()).cloned())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let params = &request["params"];
        let result: Result<Value, (i32, &str)> = match method {
            "initialize" if !initialized => {
                match (
                    params["protocolVersion"].as_str(),
                    params["clientInfo"]["name"].as_str(),
                ) {
                    (Some(version), Some(name)) if !name.trim().is_empty() && name.len() <= 128 => {
                        initialized = true;
                        client_name = name.to_owned();
                        let version = match version {
                            "2024-11-05" | "2025-03-26" | "2025-06-18" => version,
                            _ => "2025-06-18",
                        };
                        Ok(
                            json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"ArkDock","version":env!("CARGO_PKG_VERSION")},"instructions":catalog::INSTRUCTIONS}),
                        )
                    }
                    _ => Err((
                        -32602,
                        "initialize requires protocolVersion and clientInfo.name",
                    )),
                }
            }
            "ping" => Ok(json!({})),
            _ if !initialized || !ready => Err((-32002, "MCP initialization is required")),
            "initialize" => Err((-32600, "MCP is already initialized")),
            "tools/list" => Ok(json!({"tools":catalog::tools()})),
            "tools/call" => match params["name"].as_str() {
                Some(name) if catalog::known(name) => {
                    let args = params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    if !args.is_object() {
                        Err((-32602, "Tool arguments must be an object"))
                    } else if cancelled.load(Ordering::Acquire) {
                        Err((
                            -32800,
                            "Request cancelled; inspect task status before retrying",
                        ))
                    } else {
                        let result = call(&client_name, name, args, cancelled.clone());
                        if cancelled.load(Ordering::Acquire) {
                            Err((-32800, "Request cancelled; mutation outcome may be unknown"))
                        } else {
                            Ok(result)
                        }
                    }
                }
                _ => Err((-32602, "Unknown ArkDock tool")),
            },
            _ => Err((-32601, "Method not found")),
        };
        let reply = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code, message)) => error(id.clone(), code, message),
        };
        write(&mut writer, &reply)?;
        if let Ok(mut pending) = requests.lock() {
            pending.remove(&id.to_string());
        }
    }
    Ok(())
}

fn write(writer: &mut impl Write, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|_| "MCP_STDOUT_FAILED")?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|_| "MCP_STDOUT_FAILED".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn handshake() -> String {
        format!(
            "{}\n{}\n",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"Generic host","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
    }
    #[test]
    fn offline_discovery_is_protocol_only_and_has_no_backend_calls() {
        let input = format!(
            "{}{}\n",
            handshake(),
            json!({"jsonrpc":"2.0","id":"list","method":"tools/list"})
        );
        let mut output = Vec::new();
        session(input.as_bytes(), &mut output, |_, _, _| {
            panic!("offline discovery contacted desktop")
        })
        .unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "ArkDock");
        assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 9);
        assert!(!replies[0]["result"]["instructions"]
            .as_str()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn tool_errors_and_image_results_use_standard_mcp_content() {
        let input = format!(
            "{}{}\n{}\n",
            handshake(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"arkdock_get_task_image","arguments":{"contextId":"ctx","image":"crop"}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"arkdock_list_tasks"}})
        );
        let mut output = Vec::new();
        session(input.as_bytes(), &mut output, |client, name, _| {
            assert_eq!(client, "Generic host");
            if name == "arkdock_get_task_image" {
                json!({"content":[{"type":"image","mimeType":"image/png","data":"aGVsbG8="}]})
            } else {
                tool_error("MCP_DESKTOP_UNAVAILABLE")
            }
        })
        .unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies[1]["result"]["content"][0]["type"], "image");
        assert_eq!(replies[2]["result"]["isError"], true);
    }
    #[test]
    fn malformed_uninitialized_and_oversized_frames_are_bounded() {
        let mut output = Vec::new();
        session(
            b"{\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n".as_slice(),
            &mut output,
            |_, _, _| panic!(),
        )
        .unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies[0]["error"]["code"], -32700);
        assert_eq!(replies[1]["error"]["code"], -32002);
        assert_eq!(
            session(
                vec![b' '; MAX_REQUEST + 1].as_slice(),
                Vec::new(),
                |_, _, _| panic!()
            )
            .unwrap_err(),
            "MCP_FRAME_TOO_LARGE"
        );
    }
    #[test]
    fn queued_cancellation_and_eof_never_forward_tools() {
        let requests: Requests = Arc::default();
        let (send, receive) = std::sync::mpsc::sync_channel(32);
        let input = format!(
            "{}{}\n{}\n",
            handshake(),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"arkdock_list_tasks","arguments":{"contextId":"ctx"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":4}})
        );
        pump(input.as_bytes(), send, &requests);
        let reader = QueuedInput {
            receive,
            current: std::io::Cursor::new(Vec::new()),
            ended: false,
        };
        let mut output = Vec::new();
        session_with_requests(reader, &mut output, &requests, |_, _, _, _| {
            panic!("cancelled queued call reached backend")
        })
        .unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[1]["id"], 4);
        assert_eq!(replies[1]["error"]["code"], -32800);
        assert!(requests.lock().unwrap().is_empty());
    }
    #[test]
    fn cancellation_during_tool_returns_unknown_outcome_without_success() {
        let requests: Requests = Arc::default();
        let cancelled = Arc::new(AtomicBool::new(false));
        requests
            .lock()
            .unwrap()
            .insert("2".into(), cancelled.clone());
        let input = format!(
            "{}{}\n",
            handshake(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"arkdock_claim_task"}})
        );
        let mut output = Vec::new();
        session_with_requests(input.as_bytes(),&mut output,&requests,|_,_,_,active| {
            assert!(Arc::ptr_eq(&active,&cancelled)); active.store(true,Ordering::Release);
            json!({"content":[{"type":"text","text":"mutation committed just before cancellation"}]})
        }).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(replies[1].get("result").is_none());
        assert_eq!(replies[1]["error"]["code"], -32800);
        assert!(replies[1]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown"));
    }
    #[test]
    fn input_queue_and_registry_are_bounded() {
        let requests: Requests = Arc::default();
        let (send, _receive) = std::sync::mpsc::sync_channel(32);
        let input = (0..100)
            .map(|id| format!("{}\n", json!({"jsonrpc":"2.0","id":id,"method":"ping"})))
            .collect::<String>();
        pump(input.as_bytes(), send, &requests);
        let pending = requests.lock().unwrap();
        assert_eq!(pending.len(), 32);
        assert!(pending
            .values()
            .all(|cancelled| cancelled.load(Ordering::Acquire)));
    }
}
