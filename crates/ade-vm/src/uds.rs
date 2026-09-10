use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Minimal blocking HTTP/1.1 client over a unix socket.
/// Speaks just enough for the Cloud Hypervisor REST API
/// (`PUT /api/v1/vm.create`, `/vm.boot`, …, `GET /vm.info`).
/// No new deps; timeouts keep a dead daemon from hanging boot.
#[derive(Debug, Clone)]
pub struct UdsHttp {
    sock: PathBuf,
    timeout: Duration,
}

impl UdsHttp {
    pub fn new(sock: &Path) -> Self {
        Self {
            sock: sock.to_path_buf(),
            timeout: Duration::from_secs(10),
        }
    }

    pub fn put(&self, path: &str, body: Option<&str>) -> anyhow::Result<UdsResponse> {
        self.request("PUT", path, body)
    }

    pub fn get(&self, path: &str) -> anyhow::Result<UdsResponse> {
        self.request("GET", path, None)
    }

    fn request(&self, method: &str, path: &str, body: Option<&str>) -> anyhow::Result<UdsResponse> {
        let body = body.unwrap_or("");
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut s = UnixStream::connect(&self.sock)
            .map_err(|e| anyhow::anyhow!("uds connect {} failed: {e:#}", self.sock.display()))?;
        s.set_read_timeout(Some(self.timeout))?;
        s.set_write_timeout(Some(self.timeout))?;
        s.write_all(req.as_bytes())?;
        s.shutdown(std::net::Shutdown::Write).ok();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)?;
        UdsResponse::parse(&raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdsResponse {
    pub status: u16,
    pub body: String,
}

impl UdsResponse {
    pub fn parse(raw: &[u8]) -> anyhow::Result<Self> {
        let text = String::from_utf8_lossy(raw);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let first = head.lines().next().unwrap_or("");
        let status: u16 = first
            .split_whitespace()
            .nth(1)
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        if status == 0 {
            anyhow::bail!("bad http status line: {first:?}");
        }
        Ok(Self {
            status,
            body: body.to_string(),
        })
    }

    /// 2xx → Ok, else Err with the daemon's message body.
    pub fn ok(self, op: &str) -> anyhow::Result<String> {
        if (200..300).contains(&self.status) {
            Ok(self.body)
        } else {
            anyhow::bail!("{op} failed (http {}): {}", self.status, self.body.trim())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// Fake CH daemon: records the request line, replays canned responses.
    /// Unique socket per call: tests run in parallel.
    fn fake_daemon(
        responses: Vec<(u16, &'static str)>,
    ) -> (PathBuf, std::thread::JoinHandle<Vec<String>>) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_SOCK: AtomicU64 = AtomicU64::new(1);
        let sock = std::env::temp_dir().join(format!(
            "ade-uds-{}-{}",
            std::process::id(),
            NEXT_SOCK.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let seen = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for (status, body) in responses {
                let Ok((mut s, _)) = listener.accept() else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                // Read whatever arrived; the client half-closes after write.
                let mut raw = Vec::new();
                loop {
                    match s.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => raw.extend_from_slice(&buf[..n]),
                        Err(_) => break,
                    }
                }
                lines.push(
                    String::from_utf8_lossy(&raw)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                );
                let resp = format!(
                    "HTTP/1.1 {status} _\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
            lines
        });
        (sock, seen)
    }

    #[test]
    fn put_create_then_boot() {
        let (sock, seen) = fake_daemon(vec![(204, ""), (204, "")]);
        let c = UdsHttp::new(&sock);
        c.put("/api/v1/vm.create", Some(r#"{"cpus":{"boot_vcpus":2}}"#))
            .unwrap()
            .ok("vm.create")
            .unwrap();
        c.put("/api/v1/vm.boot", None)
            .unwrap()
            .ok("vm.boot")
            .unwrap();
        let lines = seen.join().unwrap();
        assert!(lines[0].starts_with("PUT /api/v1/vm.create"), "{lines:?}");
        assert!(lines[1].starts_with("PUT /api/v1/vm.boot"), "{lines:?}");
    }

    #[test]
    fn error_status_carries_body() {
        let (sock, _seen) = fake_daemon(vec![(500, "VmBoot(boom)")]);
        let c = UdsHttp::new(&sock);
        let err = c
            .put("/api/v1/vm.boot", None)
            .unwrap()
            .ok("vm.boot")
            .unwrap_err();
        assert!(err.to_string().contains("VmBoot(boom)"), "{err:#}");
    }

    #[test]
    fn get_info_body() {
        let (sock, _seen) = fake_daemon(vec![(200, r#"{"state":"Running"}"#)]);
        let c = UdsHttp::new(&sock);
        let body = c.get("/api/v1/vm.info").unwrap().ok("vm.info").unwrap();
        assert!(body.contains("Running"));
    }

    #[test]
    fn missing_socket_errors() {
        let c = UdsHttp::new(Path::new("/tmp/ade-no-such.sock"));
        assert!(c.get("/api/v1/vm.info").is_err());
    }
}
