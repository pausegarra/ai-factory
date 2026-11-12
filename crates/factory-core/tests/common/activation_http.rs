use serde_json::Value;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub authorization: String,
}
pub struct Server {
    pub base: String,
    pub calls: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub async fn start<F, Fut>(handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = (u16, Value)> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captured = calls.clone();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let calls = captured.clone();
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    loop {
                        let mut buffer = [0; 4096];
                        let n = socket.read(&mut buffer).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                        assert!(bytes.len() < 65536);
                    }
                    let raw = String::from_utf8(bytes).unwrap();
                    let mut first = raw.lines().next().unwrap().split_whitespace();
                    let request = Request {
                        method: first.next().unwrap().into(),
                        path: first.next().unwrap().into(),
                        authorization: raw
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("authorization")
                                    .then(|| value.trim().to_string())
                            })
                            .unwrap_or_default(),
                    };
                    calls.lock().unwrap().push(request.clone());
                    let (status, body) = handler(request).await;
                    let body = body.to_string();
                    let headers = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                    socket.write_all(headers.as_bytes()).await.unwrap();
                    socket.write_all(body.as_bytes()).await.unwrap();
                });
            }
        });
        Self { base, calls, task }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
