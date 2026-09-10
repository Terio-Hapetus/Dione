use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Guest CID for the single VM on this host (2 = host, 3+ = guests).
pub const GUEST_CID: u32 = 3;
/// Guest vsock port where socat forwards to sshd:22 (see ch.rs seed).
pub const GUEST_SSH_PORT: u32 = 2222;

/// Blocking AF_VSOCK stream (no async runtime needed; bridging runs on
/// plain threads). Live-gated: no vsock on KVM-less machines.
pub struct VsockStream {
    fd: RawFd,
}

impl VsockStream {
    pub fn connect(cid: u32, port: u32) -> std::io::Result<Self> {
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let addr = libc::sockaddr_vm {
            svm_family: libc::AF_VSOCK as libc::sa_family_t,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: cid,
            svm_zero: [0; 4],
        };
        let ret = unsafe {
            libc::connect(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        };
        if ret != 0 {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(e);
        }
        Ok(Self { fd })
    }
}

impl Read for VsockStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            return Ok(n as usize);
        }
    }
}

impl Write for VsockStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        loop {
            let n = unsafe { libc::write(self.fd, buf.as_ptr() as *const _, buf.len()) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            return Ok(n as usize);
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for VsockStream {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

// VsockStream is a plain fd wrapper: safe to move across threads.
unsafe impl Send for VsockStream {}

/// Pump bytes both ways until either side EOFs/errors.
fn bridge<W>(a: TcpStream, b: W)
where
    W: Read + Write + TryCloneLike + Send + 'static,
{
    let _ = a.set_nodelay(true);
    let Ok(mut a2) = a.try_clone() else { return };
    let (mut a_r, mut b_w) = (a, b);
    // Direction b→a runs on a helper thread; a→b runs here.
    let mut b_r = b_w.try_clone_like();
    let t = std::thread::spawn(move || {
        copy_loop(&mut b_r, &mut a2);
    });
    copy_loop(&mut a_r, &mut b_w);
    let _ = t.join();
}

/// Copy until EOF/error. Generic over any byte streams.
fn copy_loop<R: Read, W: Write>(r: &mut R, w: &mut W) {
    let mut buf = vec![0u8; 32 * 1024];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        if w.write_all(&buf[..n]).is_err() {
            break;
        }
    }
}

/// Handle to a running `127.0.0.1:<port> → vsock(cid:port)` forward.
/// `stop()` unblocks accept with a dummy connection and joins.
pub struct ProxyHandle {
    pub port: u16,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProxyHandle {
    pub fn stop(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake the acceptor, then join.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Listen on 127.0.0.1 (ephemeral port when `bind_port == 0`) and forward
/// each connection to `vsock(cid:vsock_port)`. Returns the bound port in
/// the handle so `wait_ssh`/`MicroVm` can target it.
pub fn forward_tcp_to_vsock(
    bind_port: u16,
    cid: u32,
    vsock_port: u32,
) -> std::io::Result<ProxyHandle> {
    let listener = TcpListener::bind(("127.0.0.1", bind_port))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(false)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = shutdown.clone();
    let thread = std::thread::spawn(move || {
        listener
            .set_nonblocking(true)
            .expect("proxy listener nonblocking");
        loop {
            if flag.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((client, _)) => {
                    if flag.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::spawn(move || {
                        let Ok(guest) = VsockStream::connect(cid, vsock_port) else {
                            return;
                        };
                        bridge(client, guest);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    Ok(ProxyHandle {
        port,
        shutdown,
        thread: Some(thread),
    })
}

/// `try_clone` for generic W in bridge(): TcpStream and VsockStream both
/// support it via duplication; UnixStream too (used by tests).
trait TryCloneLike: Sized {
    fn try_clone_like(&self) -> Self;
}

impl TryCloneLike for TcpStream {
    fn try_clone_like(&self) -> Self {
        self.try_clone().expect("tcp try_clone")
    }
}

impl TryCloneLike for VsockStream {
    fn try_clone_like(&self) -> Self {
        let fd = unsafe { libc::dup(self.fd) };
        assert!(fd >= 0, "vsock dup failed");
        Self { fd }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    impl TryCloneLike for UnixStream {
        fn try_clone_like(&self) -> Self {
            self.try_clone().expect("unix try_clone")
        }
    }

    /// Echo server on a unix socket: validates the byte pump without vsock.
    #[test]
    fn bridge_echoes_both_directions() {
        let (a, b) = UnixStream::pair().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (client, _) = listener.accept().unwrap();
            bridge(client, b);
        });
        // Echo end: read a line, write it back, twice (both directions share it).
        let (mut echo_r, mut echo_w) = (a.try_clone().unwrap(), a);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64];
            for _ in 0..2 {
                let n = echo_r.read(&mut buf).unwrap();
                echo_w.write_all(&buf[..n]).unwrap();
            }
        });
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(b"ping").unwrap();
        let mut back = [0u8; 4];
        c.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"ping");
        c.write_all(b"pong").unwrap();
        c.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"pong");
    }

    #[test]
    fn proxy_handle_binds_ephemeral() {
        let h = forward_tcp_to_vsock(0, GUEST_CID, GUEST_SSH_PORT).unwrap();
        assert!(h.port > 0);
        // Accept loop is up: a connect succeeds even with no guest.
        assert!(TcpStream::connect(("127.0.0.1", h.port)).is_ok());
        h.stop();
    }
}
