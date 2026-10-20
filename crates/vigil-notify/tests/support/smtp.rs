//! A small plaintext SMTP server for tests. It does not offer STARTTLS.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// One message the server accepted.
#[derive(Debug, Clone, Default)]
pub struct Received {
    pub from: String,
    pub rcpts: Vec<String>,
    pub data: String,
    pub auth: Option<(String, String)>,
}

/// How the server answers `RCPT TO`.
#[derive(Debug, Clone, Copy)]
pub enum Rcpt {
    Accept,
    /// Answer 451 this many times, then accept.
    TemporaryFailures(usize),
    /// Always answer 550.
    Reject,
}

struct Shared {
    rcpt: Rcpt,
    failures_left: AtomicUsize,
    received: Mutex<Vec<Received>>,
    connections: AtomicUsize,
}

pub struct SmtpServer {
    pub port: u16,
    shared: Arc<Shared>,
}

impl SmtpServer {
    pub fn received(&self) -> Vec<Received> {
        self.shared
            .received
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }
}

pub async fn start(rcpt: Rcpt) -> SmtpServer {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("could not bind the test SMTP server: {error}"),
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(error) => panic!("no address for the test SMTP server: {error}"),
    };
    let failures_left = AtomicUsize::new(match rcpt {
        Rcpt::TemporaryFailures(n) => n,
        _ => 0,
    });
    let shared = Arc::new(Shared {
        rcpt,
        failures_left,
        received: Mutex::new(Vec::new()),
        connections: AtomicUsize::new(0),
    });
    let server = Arc::clone(&shared);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            server.connections.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(serve(stream, Arc::clone(&server)));
        }
    });
    SmtpServer { port, shared }
}

async fn serve(stream: TcpStream, shared: Arc<Shared>) {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let mut current = Received::default();
    let mut auth = None;
    if write.write_all(b"220 test ESMTP\r\n").await.is_err() {
        return;
    }
    while let Some(line) = next_line(&mut read).await {
        let upper = line.to_ascii_uppercase();
        let reply: String = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            "250-test\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n".into()
        } else if upper.starts_with("AUTH PLAIN") {
            let mut response = line[10..].trim().to_owned();
            if response.is_empty() {
                let _ = write.write_all(b"334 \r\n").await;
                response = next_line(&mut read).await.unwrap_or_default();
            }
            let decoded = base64_decode(response.trim());
            let mut parts = decoded.split('\0').skip(1);
            auth = Some((
                parts.next().unwrap_or_default().to_owned(),
                parts.next().unwrap_or_default().to_owned(),
            ));
            "235 ok\r\n".into()
        } else if upper.starts_with("AUTH LOGIN") {
            let _ = write.write_all(b"334 VXNlcm5hbWU6\r\n").await;
            let user = base64_decode(next_line(&mut read).await.unwrap_or_default().trim());
            let _ = write.write_all(b"334 UGFzc3dvcmQ6\r\n").await;
            let pass = base64_decode(next_line(&mut read).await.unwrap_or_default().trim());
            auth = Some((user, pass));
            "235 ok\r\n".into()
        } else if upper.starts_with("MAIL FROM:") {
            current = Received::default();
            current.from = address(&line);
            "250 ok\r\n".into()
        } else if upper.starts_with("RCPT TO:") {
            match shared.rcpt {
                Rcpt::Reject => "550 mailbox unavailable\r\n".into(),
                Rcpt::TemporaryFailures(_)
                    if shared
                        .failures_left
                        .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                        .is_ok() =>
                {
                    "451 try again later\r\n".into()
                }
                _ => {
                    current.rcpts.push(address(&line));
                    "250 ok\r\n".into()
                }
            }
        } else if upper == "DATA" {
            let _ = write.write_all(b"354 go ahead\r\n").await;
            let mut data = String::new();
            while let Some(l) = next_line(&mut read).await {
                if l == "." {
                    break;
                }
                data.push_str(&l);
                data.push('\n');
            }
            current.data = data;
            current.auth = auth.clone();
            if let Ok(mut all) = shared.received.lock() {
                all.push(current.clone());
            }
            "250 queued\r\n".into()
        } else if upper == "QUIT" {
            let _ = write.write_all(b"221 bye\r\n").await;
            return;
        } else {
            "250 ok\r\n".into()
        };
        if write.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// The next line without its line ending, or `None` when the client has gone.
async fn next_line(read: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Option<String> {
    let mut line = String::new();
    match read.read_line(&mut line).await {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_owned()),
    }
}

/// `a@b.c` out of `MAIL FROM:<a@b.c> SIZE=1`.
fn address(line: &str) -> String {
    let start = line.find('<').map_or(0, |i| i + 1);
    let end = line.find('>').unwrap_or(line.len());
    line[start..end].to_owned()
}

fn base64_decode(text: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::new();
    let (mut bits, mut have) = (0u32, 0);
    for c in text.bytes().filter(|c| *c != b'=') {
        let Some(value) = ALPHABET.iter().position(|a| *a == c) else {
            continue;
        };
        bits = (bits << 6) | value as u32;
        have += 6;
        if have >= 8 {
            have -= 8;
            bytes.push((bits >> have) as u8);
            bits &= (1 << have) - 1;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
