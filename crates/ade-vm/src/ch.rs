use std::collections::{BTreeMap, BTreeSet};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::backend::{VmBackend, VmError};
use super::config::{SshInfo, VmConfig, VmHandle, VmState};
use super::image::{ImageSpec, ensure_image};
use super::seed::{FS_TAG, build_seed_iso};
use super::uds::UdsHttp;
use super::vsock_proxy::{GUEST_CID, GUEST_SSH_PORT, ProxyHandle, forward_tcp_to_vsock};

const API_PATH: &str = "/api/v1";

/// Native backend: `cloud-hypervisor` daemon driven over its REST API
/// (unix socket) + `virtiofsd` for the repo mount + vsock ssh proxy.
/// Needs on the live machine: KVM, `cloud-hypervisor`, `virtiofsd`,
/// `genisoimage`, and guest assets (see [`resolve_assets`]).
/// The `ADE_LIVE_VM=1` test is the final validator of every flag below.
#[derive(Debug)]
pub struct CloudHypervisorBackend {
    ch_bin: PathBuf,
    virtiofsd_bin: PathBuf,
    vm_dir: PathBuf,
    vms: BTreeMap<String, RunningVm>,
    stopped: BTreeSet<String>,
    next_id: AtomicU64,
}

#[derive(Debug)]
struct RunningVm {
    api_sock: PathBuf,
    ch_child: Child,
    virtiofs_child: Child,
    proxy: Option<ProxyHandle>,
    ssh_port: u16,
}

impl CloudHypervisorBackend {
    pub fn new(vm_dir: PathBuf) -> Self {
        Self::with_bins(
            PathBuf::from("cloud-hypervisor"),
            PathBuf::from("virtiofsd"),
            vm_dir,
        )
    }

    pub fn with_bins(ch_bin: PathBuf, virtiofsd_bin: PathBuf, vm_dir: PathBuf) -> Self {
        Self {
            ch_bin,
            virtiofsd_bin,
            vm_dir,
            vms: BTreeMap::new(),
            stopped: BTreeSet::new(),
            next_id: AtomicU64::new(1),
        }
    }

    fn vm_id(&mut self, repo: &Path) -> String {
        let base: String = repo
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("ws")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_lowercase();
        let base = if base.is_empty() { "ws".into() } else { base };
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("ch-{base}-{n}")
    }
}

impl VmBackend for CloudHypervisorBackend {
    fn boot(&mut self, cfg: &VmConfig) -> anyhow::Result<VmHandle> {
        let id = self.vm_id(&cfg.mount_repo);
        let workdir = self.vm_dir.join(&id);
        std::fs::create_dir_all(&workdir)?;
        // Best-effort cleanup closure on failure paths below.
        let fail = |msg: String| {
            let _ = std::fs::remove_dir_all(&workdir);
            anyhow::anyhow!("{msg}")
        };

        let (kernel, image) = resolve_assets(&self.vm_dir).map_err(|e| fail(format!("{e:#}")))?;
        let seed = build_seed_iso(&workdir.join("seed"), &id, &cfg.ssh_pubkey, GUEST_SSH_PORT)
            .map_err(|e| fail(format!("seed iso: {e:#}")))?;

        let virtiofs_sock = workdir.join("virtiofs.sock");
        let mut virtiofs_child = spawn_checked(
            &self.virtiofsd_bin,
            &[
                "--socket-path",
                &virtiofs_sock.to_string_lossy(),
                "--shared-dir",
                &cfg.mount_repo.to_string_lossy(),
            ],
        )
        .map_err(|e| fail(format!("virtiofsd spawn: {e:#}")))?;
        // NOTE: virtiofsd flag set varies by build; live-verify and adjust.

        let api_sock = workdir.join("api.sock");
        let mut ch_child =
            spawn_checked(&self.ch_bin, &["--api-socket", &api_sock.to_string_lossy()]).map_err(
                |e| {
                    kill(&mut virtiofs_child);
                    fail(format!("cloud-hypervisor spawn: {e:#}"))
                },
            )?;

        if !wait_for_file(&api_sock, Duration::from_secs(10)) {
            kill(&mut ch_child);
            kill(&mut virtiofs_child);
            return Err(fail("api socket never appeared".into()));
        }
        let api = UdsHttp::new(&api_sock);
        let create = build_vm_config(cfg, &kernel, &image, &seed, &virtiofs_sock);
        if let Err(e) = api
            .put(&format!("{API_PATH}/vm.create"), Some(&create.to_string()))
            .and_then(|r| r.ok("vm.create"))
        {
            kill(&mut ch_child);
            kill(&mut virtiofs_child);
            return Err(fail(format!("{e:#}")));
        }
        if let Err(e) = api
            .put(&format!("{API_PATH}/vm.boot"), None)
            .and_then(|r| r.ok("vm.boot"))
        {
            kill(&mut ch_child);
            kill(&mut virtiofs_child);
            return Err(fail(format!("{e:#}")));
        }

        let proxy = forward_tcp_to_vsock(0, GUEST_CID, GUEST_SSH_PORT).map_err(|e| {
            kill(&mut ch_child);
            kill(&mut virtiofs_child);
            fail(format!("vsock proxy: {e:#}"))
        })?;
        let ssh_port = proxy.port;
        self.vms.insert(
            id.clone(),
            RunningVm {
                api_sock,
                ch_child,
                virtiofs_child,
                proxy: Some(proxy),
                ssh_port,
            },
        );
        self.stopped.remove(&id);
        Ok(VmHandle { id })
    }

    fn wait_ssh(&self, handle: &VmHandle) -> anyhow::Result<SshInfo> {
        let vm = self
            .vms
            .get(&handle.id)
            .ok_or_else(|| VmError::UnknownHandle(handle.id.clone()))?;
        // One probe; VmManager polls until its ssh timeout.
        let addr: SocketAddr = format!("127.0.0.1:{}", vm.ssh_port).parse().unwrap();
        TcpStream::connect_timeout(&addr, Duration::from_secs(2))
            .map_err(|e| anyhow::anyhow!("ssh proxy 127.0.0.1:{} not ready: {e:#}", vm.ssh_port))?;
        Ok(SshInfo {
            host: "127.0.0.1".into(),
            port: vm.ssh_port,
            user: "ubuntu".into(),
        })
    }

    fn state_of(&self, handle: &VmHandle) -> VmState {
        if self.stopped.contains(&handle.id) {
            return VmState::Stopped;
        }
        let Some(vm) = self.vms.get(&handle.id) else {
            return VmState::Missing;
        };
        match UdsHttp::new(&vm.api_sock)
            .get(&format!("{API_PATH}/vm.info"))
            .and_then(|r| r.ok("vm.info"))
        {
            Ok(body) => map_ch_state(&body),
            Err(e) => VmState::Error(format!("vm.info: {e:#}")),
        }
    }

    fn stop(&mut self, handle: &VmHandle) -> anyhow::Result<()> {
        if self.stopped.contains(&handle.id) {
            return Err(VmError::AlreadyStopped(handle.id.clone()).into());
        }
        let Some(mut vm) = self.vms.remove(&handle.id) else {
            return Err(VmError::UnknownHandle(handle.id.clone()).into());
        };
        let api = UdsHttp::new(&vm.api_sock);
        let _ = api
            .put(&format!("{API_PATH}/vm.shutdown"), None)
            .and_then(|r| r.ok("vm.shutdown"));
        // Brief grace period, then delete + kill unconditionally.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if matches!(self.info_state(&vm), Some(s) if s == "Shutdown") {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = api
            .put(&format!("{API_PATH}/vm.delete"), None)
            .and_then(|r| r.ok("vm.delete"));
        kill(&mut vm.ch_child);
        kill(&mut vm.virtiofs_child);
        if let Some(proxy) = vm.proxy.take() {
            proxy.stop();
        }
        self.stopped.insert(handle.id.clone());
        Ok(())
    }
}

impl CloudHypervisorBackend {
    fn info_state(&self, vm: &RunningVm) -> Option<String> {
        let body = UdsHttp::new(&vm.api_sock)
            .get(&format!("{API_PATH}/vm.info"))
            .and_then(|r| r.ok("vm.info"))
            .ok()?;
        serde_json::from_str::<serde_json::Value>(&body)
            .ok()?
            .get("state")?
            .as_str()
            .map(str::to_string)
    }
}

impl Drop for CloudHypervisorBackend {
    fn drop(&mut self) {
        // Best-effort: never leave daemons behind on panic paths.
        for (_, mut vm) in std::mem::take(&mut self.vms) {
            kill(&mut vm.ch_child);
            kill(&mut vm.virtiofs_child);
        }
    }
}

/// Guest assets: local paths win, else pinned download, else clear error.
/// Env: ADE_VM_KERNEL / ADE_VM_IMAGE, or ADE_VM_KERNEL_URL (+_SHA256),
/// ADE_VM_IMAGE_URL (+_SHA256). Production must pin SHA256.
pub fn resolve_assets(cache_dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let kernel = resolve_one(
        "kernel",
        "ADE_VM_KERNEL",
        "ADE_VM_KERNEL_URL",
        "ADE_VM_KERNEL_SHA256",
        "vmlinux",
        cache_dir,
    )?;
    let image = resolve_one(
        "image",
        "ADE_VM_IMAGE",
        "ADE_VM_IMAGE_URL",
        "ADE_VM_IMAGE_SHA256",
        "rootfs.img",
        cache_dir,
    )?;
    Ok((kernel, image))
}

fn resolve_one(
    what: &str,
    path_var: &str,
    url_var: &str,
    sha_var: &str,
    filename: &str,
    cache_dir: &Path,
) -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var(path_var) {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        anyhow::bail!(
            "{path_var}={} missing; fix the path or unset it",
            p.display()
        );
    }
    let Ok(url) = std::env::var(url_var) else {
        anyhow::bail!(
            "no {what}: set {path_var} to a local file or {url_var}(+{sha_var}) to download"
        );
    };
    let sha256 = std::env::var(sha_var).unwrap_or_else(|_| "skip".into());
    ensure_image(
        &ImageSpec {
            url,
            sha256,
            filename: filename.into(),
            max_time_secs: ImageSpec::DEFAULT_MAX_TIME_SECS,
        },
        cache_dir,
    )
}

/// Pure vm.create payload (unit-tested here; daemon-tested live).
pub fn build_vm_config(
    cfg: &VmConfig,
    kernel: &Path,
    image: &Path,
    seed_iso: &Path,
    virtiofs_sock: &Path,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "cpus": {"boot_vcpus": cfg.vcpu},
        "memory": {"size": cfg.mem_mb as u64 * 1024 * 1024},
        "payload": {"kernel": kernel.to_string_lossy(), "cmdline": "console=hvc0 root=/dev/vda1 rw"},
        "disks": [{"path": image.to_string_lossy()}, {"path": seed_iso.to_string_lossy()}],
        "fs": [{"tag": FS_TAG, "socket": virtiofs_sock.to_string_lossy(), "num_queues": 1, "queue_size": 1024}],
        "rng": {"src": "/dev/urandom"},
        "vsock": {"cid": GUEST_CID},
    });
    // Escape hatch for vsock setups needing an explicit host socket.
    if let Ok(sock) = std::env::var("ADE_VM_VSOCK_SOCK") {
        payload["vsock"]["socket"] = serde_json::Value::String(sock);
    }
    // FS_TAG contract with seed.rs user-data + MicroVm Mounted mapping.
    debug_assert_eq!(FS_TAG, "workspace");
    payload
}

pub(crate) fn map_ch_state(body: &str) -> VmState {
    let state = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("state")?.as_str().map(str::to_string));
    match state.as_deref() {
        Some("Running") => VmState::Running,
        Some("Created") => VmState::Booting,
        Some("Shutdown") => VmState::Stopped,
        Some(other) => VmState::Error(format!("unexpected vm state: {other}")),
        None => VmState::Error("vm.info has no state field".into()),
    }
}

fn spawn_checked(bin: &Path, args: &[&str]) -> anyhow::Result<Child> {
    Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn {} failed: {e:#}", bin.display()))
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_payload_shape() {
        let cfg = VmConfig {
            vcpu: 2,
            mem_mb: 2048,
            ..VmConfig::default()
        };
        let v = build_vm_config(
            &cfg,
            Path::new("/cache/vmlinux"),
            Path::new("/cache/rootfs.img"),
            Path::new("/vm/seed.iso"),
            Path::new("/vm/virtiofs.sock"),
        );
        assert_eq!(v["cpus"]["boot_vcpus"], 2);
        assert_eq!(v["memory"]["size"], 2048u64 * 1024 * 1024);
        assert_eq!(v["fs"][0]["tag"], "workspace");
        assert_eq!(v["vsock"]["cid"], GUEST_CID);
        assert_eq!(v["disks"].as_array().unwrap().len(), 2);
        assert!(
            v["payload"]["cmdline"]
                .as_str()
                .unwrap()
                .contains("console=hvc0")
        );
    }

    #[test]
    fn state_mapping() {
        assert_eq!(map_ch_state(r#"{"state":"Running"}"#), VmState::Running);
        assert_eq!(map_ch_state(r#"{"state":"Created"}"#), VmState::Booting);
        assert_eq!(map_ch_state(r#"{"state":"Shutdown"}"#), VmState::Stopped);
        assert!(matches!(map_ch_state(r#"{"nope":1}"#), VmState::Error(_)));
    }

    #[test]
    fn missing_assets_error_is_actionable() {
        unsafe {
            std::env::remove_var("ADE_VM_KERNEL");
            std::env::remove_var("ADE_VM_IMAGE");
            std::env::remove_var("ADE_VM_KERNEL_URL");
            std::env::remove_var("ADE_VM_IMAGE_URL");
        }
        let err = resolve_assets(Path::new("/tmp/ade-nope")).unwrap_err();
        assert!(err.to_string().contains("ADE_VM_KERNEL"), "{err:#}");
    }

    #[test]
    fn missing_binaries_fail_fast() {
        let dir = std::env::temp_dir().join("ade-ch-probe");
        let mut b = CloudHypervisorBackend::with_bins(
            PathBuf::from("/nonexistent/ch"),
            PathBuf::from("/nonexistent/virtiofsd"),
            dir,
        );
        // Fails on assets first (no env set): fast, no hang, no daemon.
        let err = b.boot(&VmConfig::default()).unwrap_err();
        assert!(err.to_string().contains("ADE_VM_"), "{err:#}");
        let h = VmHandle { id: "nope".into() };
        assert_eq!(b.state_of(&h), VmState::Missing);
        assert!(b.stop(&h).is_err());
    }
}
