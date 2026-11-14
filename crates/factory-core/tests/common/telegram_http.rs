use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
#[derive(Clone)]
pub struct Request {
    pub method: String,
    pub content_type: String,
    pub path: String,
    pub body: serde_json::Value,
}
pub struct Server {
    pub base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    task: JoinHandle<()>,
}
impl Server {
    pub async fn json_response(status: u16, body: serde_json::Value) -> Self {
        Self::raw(format!("HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.to_string().len()).into_bytes(),0).await
    }
    pub async fn raw(response: Vec<u8>, delay_ms: u64) -> Self {
        Self::parts(response, delay_ms, None).await
    }
    pub async fn delayed_body(response: Vec<u8>, delay_ms: u64) -> Self {
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|n| n + 4);
        Self::parts(response, delay_ms, split).await
    }
    async fn parts(response: Vec<u8>, delay_ms: u64, split: Option<usize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            // One connection per fixture; aborting this task also drops the stream.
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let n = stream.read(&mut buffer).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let len = headers
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() < end + 4 + len {
                        continue;
                    }
                    let mut line = headers.lines().next().unwrap().split_whitespace();
                    captured.lock().unwrap().push(Request {
                        method: line.next().unwrap().into(),
                        content_type: headers
                            .lines()
                            .find_map(|l| {
                                l.split_once(':')
                                    .filter(|(key, _)| key.eq_ignore_ascii_case("content-type"))
                                    .map(|(_, value)| value.trim().to_owned())
                            })
                            .unwrap_or_default(),
                        path: line.next().unwrap().into(),
                        body: serde_json::from_slice(&bytes[end + 4..end + 4 + len]).unwrap(),
                    });
                    break;
                }
            }
            if let Some(split) = split {
                let _ = stream.write_all(&response[..split]).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            let _ = stream.write_all(&response[split.unwrap_or(0)..]).await;
        });
        Self {
            base,
            requests,
            task,
        }
    }
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
    pub async fn shutdown(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
