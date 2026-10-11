use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub(super) fn start() -> Result<SessionBridge, String> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "AGENT_BRIDGE_BIND_FAILED")?;
    let port = listener
        .local_addr()
        .map_err(|_| "AGENT_BRIDGE_BIND_FAILED")?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|_| "AGENT_BRIDGE_BIND_FAILED")?;
    let shared = Arc::new(Mutex::new(Shared::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_shared = shared.clone();
    let worker_stop = stop.clone();
    let worker=std::thread::Builder::new().name("arkdock-agent-bridge".into()).spawn(move|| {
        while !worker_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream,address)) if address.ip().is_loopback()=> {
                    if stream.set_nonblocking(false).is_err() { continue; }
                    let _=stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let _=stream.set_write_timeout(Some(Duration::from_millis(500)));
                    let result=read_request(&mut stream,port).and_then(|(token,route,value)| {
                        let mut guard=worker_shared.lock().map_err(|_|"AGENT_BRIDGE_UNAVAILABLE")?;
                        handle(&mut guard,&token,&route,value,crate::selection::now_ms())
                    });
                    let rejected=result.is_err();
                    let (code,body)=match result {Ok(body)=>(200,body),Err(error)=>(400,json!({"error":error}))};
                    let body=body.to_string();
                    let _=write!(stream,"HTTP/1.1 {code} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",body.len());
                    if rejected { drain_rejected_body(&mut stream); }
                }
                Ok(_)=>{},
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(10)),
                Err(_)=>break,
            }
        }
    }).map_err(|_|"AGENT_BRIDGE_THREAD_FAILED")?;
    Ok(SessionBridge {
        shared,
        stop,
        worker: Some(worker),
        endpoint: format!("http://127.0.0.1:{port}"),
    })
}

fn drain_rejected_body(stream: &mut TcpStream) {
    // Windows may discard the error response with RST when unread request bytes
    // remain. Half-close the response, then discard only a bounded remainder.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let started = std::time::Instant::now();
    let mut left = 136 * 1024;
    let mut buffer = [0u8; 4096];
    while left > 0 && started.elapsed() < Duration::from_millis(150) {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => left -= count.min(left),
        }
    }
}

fn read_request(stream: &mut TcpStream, port: u16) -> Result<(String, String, Value), String> {
    let started = std::time::Instant::now();
    let mut bytes = Vec::new();
    let header_end;
    loop {
        if started.elapsed() > Duration::from_secs(2) {
            return Err("AGENT_REQUEST_DEADLINE".into());
        }
        if bytes.len() > 8192 {
            return Err("AGENT_HEADER_LIMIT".into());
        }
        let mut byte = [0u8; 1];
        stream
            .read_exact(&mut byte)
            .map_err(|_| "AGENT_REQUEST_READ_FAILED")?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            header_end = bytes.len();
            break;
        }
    }
    let headers = std::str::from_utf8(&bytes[..header_end]).map_err(|_| "AGENT_HEADER_INVALID")?;
    let mut lines = headers.split("\r\n");
    let parts = lines
        .next()
        .ok_or("AGENT_REQUEST_INVALID")?
        .split(' ')
        .collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "POST" || parts[2] != "HTTP/1.1" {
        return Err("AGENT_POST_REQUIRED".into());
    }
    let route = parts[1].to_owned();
    let mut map = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').ok_or("AGENT_HEADER_INVALID")?;
        if map
            .insert(key.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err("AGENT_DUPLICATE_HEADER".into());
        }
    }
    if map.contains_key("origin")
        || map.contains_key("transfer-encoding")
        || map.contains_key("cookie")
        || map.get("host") != Some(&format!("127.0.0.1:{port}"))
    {
        return Err("AGENT_REQUEST_ORIGIN_REJECTED".into());
    }
    if map
        .get("content-type")
        .is_none_or(|value| value != "application/json")
    {
        return Err("AGENT_JSON_REQUIRED".into());
    }
    let token = map
        .get("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 64)
        .ok_or("AGENT_UNAUTHORIZED")?
        .to_owned();
    let length = map
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n <= 128 * 1024)
        .ok_or("AGENT_BODY_LIMIT")?;
    let mut body = vec![0u8; length];
    let mut offset = 0;
    while offset < length {
        if started.elapsed() > Duration::from_secs(2) {
            return Err("AGENT_REQUEST_DEADLINE".into());
        }
        let read = stream
            .read(&mut body[offset..])
            .map_err(|_| "AGENT_BODY_READ_FAILED")?;
        if read == 0 {
            return Err("AGENT_BODY_READ_FAILED".into());
        }
        offset += read;
    }
    let value = serde_json::from_slice(&body).map_err(|_| "AGENT_JSON_INVALID")?;
    Ok((token, route, value))
}
