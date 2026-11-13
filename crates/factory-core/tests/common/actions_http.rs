use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[allow(dead_code)]
pub enum Reply {
    Json(u16, Value),
    Raw(u16, Vec<u8>),
    Disconnect,
    Wait(Arc<tokio::sync::Notify>, Value),
}
pub struct Step {
    method: String,
    path: String,
    body: Option<Value>,
    reply: Reply,
    hook: Option<Box<dyn FnOnce() + Send>>,
    header: Option<(String, String)>,
}
impl Step {
    pub fn new(method: &str, path: &str, reply: Reply) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            body: None,
            reply,
            hook: None,
            header: None,
        }
    }
    #[allow(dead_code)]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.header = Some((name.into(), value.into()));
        self
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
    requests: Vec<(String, String, Value)>,
    errors: Vec<String>,
}
pub struct Script {
    pub base: String,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
    stop: tokio_util::sync::CancellationToken,
}
impl Script {
    pub async fn start(steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            steps: steps.into(),
            calls: vec![],
            requests: vec![],
            errors: vec![],
        }));
        let shared = state.clone();
        let stop = tokio_util::sync::CancellationToken::new();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            loop {
                let (mut socket, _) = tokio::select! {
                    _ = task_stop.cancelled() => break,
                    result=listener.accept()=>match result {Ok(socket)=>socket,Err(_)=>break},
                    result=handlers.join_next(), if !handlers.is_empty()=>{if result.is_some_and(|r|r.is_err()){shared.lock().unwrap().errors.push("HTTP handler panicked".into());} continue;}
                };
                let shared = shared.clone();
                handlers.spawn(async move {
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
                        state.requests.push((method.clone(),path.clone(),body.clone()));
                        match state.steps.pop_front() {
                            Some(step)
                                if step.method == method
                                    && step.path == path
                                    && step.body.as_ref().is_none_or(|v| v == &body)
                                    && step.header.as_ref().is_none_or(|(name,value)| headers.lines().filter_map(|line|line.split_once(':')).any(|(n,v)|n.eq_ignore_ascii_case(name)&&v.trim()==value)) =>
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
                                Reply::Wait(gate,value) => {gate.notified().await;(201,value.to_string().into_bytes())},
                            }
                        }
                        None => (500, b"{}".to_vec()),
                    };
                    let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len());
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(&data).await;
                });
            }
            handlers.abort_all();
            while let Some(result) = handlers.join_next().await {
                if result.is_err_and(|error| !error.is_cancelled()) {
                    shared
                        .lock()
                        .unwrap()
                        .errors
                        .push("HTTP handler panicked".into());
                }
            }
        });
        Self {
            base,
            state,
            task,
            stop,
        }
    }
    #[allow(dead_code)]
    pub async fn shutdown(&mut self) {
        self.stop.cancel();
        let _ = (&mut self.task).await;
    }
    #[allow(dead_code)]
    pub fn requests(&self) -> Vec<(String, String, Value)> {
        self.state.lock().unwrap().requests.clone()
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
