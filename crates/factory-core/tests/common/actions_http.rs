use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub enum Reply {
    Json(u16, Value),
    Raw(u16, Vec<u8>),
    Disconnect,
}
pub struct Step {
    method: String,
    path: String,
    body: Option<Value>,
    reply: Reply,
    hook: Option<Box<dyn FnOnce() + Send>>,
}
impl Step {
    pub fn new(method: &str, path: &str, reply: Reply) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            body: None,
            reply,
            hook: None,
        }
    }
    pub fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
    pub fn then(mut self, hook: impl FnOnce() + Send + 'static) -> Self {
        self.hook = Some(Box::new(hook));
        self
    }
}
struct State {
    steps: VecDeque<Step>,
    calls: Vec<(String, String)>,
    errors: Vec<String>,
}
pub struct Script {
    pub base: String,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}
impl Script {
    pub async fn start(steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            steps: steps.into(),
            calls: vec![],
            errors: vec![],
        }));
        let shared = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let shared = shared.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let (start, length) = loop {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|n| n.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            break (end + 4, length);
                        }
                    };
                    while bytes.len() < start + length {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    let headers = String::from_utf8_lossy(&bytes[..start]);
                    let mut first = headers.lines().next().unwrap().split_whitespace();
                    let method = first.next().unwrap().to_owned();
                    let path = first.next().unwrap().to_owned();
                    let body = if length == 0 {
                        Value::Null
                    } else {
                        serde_json::from_slice(&bytes[start..start + length]).unwrap()
                    };
                    let step = {
                        let mut state = shared.lock().unwrap();
                        state.calls.push((method.clone(), path.clone()));
                        match state.steps.pop_front() {
                            Some(step)
                                if step.method == method
                                    && step.path == path
                                    && step.body.as_ref().is_none_or(|v| v == &body) =>
                            {
                                Some(step)
                            }
                            Some(step) => {
                                state.errors.push(format!(
                                    "Expected {} {}, got {method} {path}",
                                    step.method, step.path
                                ));
                                None
                            }
                            None => {
                                state.errors.push(format!("Unexpected {method} {path}"));
                                None
                            }
                        }
                    };
                    let (status, data) = match step {
                        Some(mut step) => {
                            if let Some(hook) = step.hook.take() {
                                hook();
                            }
                            match step.reply {
                                Reply::Json(status, value) => {
                                    (status, value.to_string().into_bytes())
                                }
                                Reply::Raw(status, data) => (status, data),
                                Reply::Disconnect => return,
                            }
                        }
                        None => (500, b"{}".to_vec()),
                    };
                    let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len());
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(&data).await;
                });
            }
        });
        Self { base, state, task }
    }
    pub fn assert_done(&self) {
        let state = self.state.lock().unwrap();
        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert!(
            state.steps.is_empty(),
            "{} unconsumed requests",
            state.steps.len()
        );
    }
    pub fn writes(&self) -> Vec<(String, String)> {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(method, _)| method != "GET")
            .cloned()
            .collect()
    }
}
impl Drop for Script {
    fn drop(&mut self) {
        self.task.abort();
    }
}
