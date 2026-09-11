//! M6e kit tests: `kits/opencode.sh` against fake guest binaries.
//!
//! Host-only, no network: a fake bindir provides `opencode` (version from
//! a file) and `npm` (writes the version file + materialises the stub,
//! like a real global install). The no-npm-latest branch would `curl`,
//! so only the deterministic pinned branches run here; the live guest
//! path is covered by `ADE_LIVE_KIT=1` on a KVM machine (Lab 2 setup).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn kit_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../kits/opencode.sh")
        .canonicalize()
        .expect("kits/opencode.sh exists")
}

fn uniq(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("ade-kit-{name}-{}-{n}", std::process::id()))
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// Fake guest: `bindir/opencode` echoes `$FAKE_VERSION_FILE`,
/// `bindir/npm` installs `opencode-ai@<v>` by writing that file (+ `latest`
/// resolves to 9.9.9). `sysdir` holds just head/tr so the real npm/curl
/// on this host never leak into the test PATH.
struct FakeGuest {
    bindir: PathBuf,
    version_file: PathBuf,
}

impl FakeGuest {
    fn new(version: &str) -> Self {
        let root = uniq("guest");
        let bindir = root.join("bin");
        let sysdir = root.join("sys");
        std::fs::create_dir_all(&bindir).unwrap();
        std::fs::create_dir_all(&sysdir).unwrap();
        for tool in ["cat", "head", "tr"] {
            std::os::unix::fs::symlink(format!("/usr/bin/{tool}"), sysdir.join(tool)).unwrap();
        }
        let version_file = root.join("VERSION");
        std::fs::write(&version_file, version).unwrap();
        write_exe(
            &bindir.join("opencode"),
            "#!/bin/sh\ncat \"$FAKE_VERSION_FILE\" 2>/dev/null\n",
        );
        write_exe(
            &bindir.join("npm"),
            r#"#!/bin/sh
# fake: npm install -g opencode-ai@<v>  (last arg wins)
spec=""
for a in "$@"; do spec="$a"; done
v="${spec##*@}"
if [ "$v" = "latest" ]; then v="9.9.9"; fi
echo "$v" > "$FAKE_VERSION_FILE"
"#,
        );
        Self {
            bindir,
            version_file,
        }
    }

    fn path(&self, with_npm: bool) -> String {
        let sys = self.bindir.parent().unwrap().join("sys");
        if with_npm {
            format!("{}:{}", self.bindir.display(), sys.display())
        } else {
            // opencode stub only: hide the npm stub behind a filtered dir.
            let nobin = self.bindir.parent().unwrap().join("nobin");
            std::fs::create_dir_all(&nobin).unwrap();
            std::os::unix::fs::symlink(self.bindir.join("opencode"), nobin.join("opencode")).ok();
            format!("{}:{}", nobin.display(), sys.display())
        }
    }

    fn run(&self, want: Option<&str>, with_npm: bool) -> (bool, String) {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(kit_path());
        // Never inherit the outer env (a real npm/curl must not resolve).
        cmd.env_clear();
        cmd.env("PATH", self.path(with_npm));
        cmd.env(
            "FAKE_VERSION_FILE",
            self.version_file.to_string_lossy().as_ref(),
        );
        if let Some(v) = want {
            cmd.env("ADE_OPENCODE_VERSION", v);
        }
        let out = cmd.output().expect("run kit script");
        let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
        combined.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), combined)
    }

    fn installed(&self) -> String {
        std::fs::read_to_string(&self.version_file)
            .unwrap()
            .trim()
            .to_string()
    }
}

#[test]
fn matching_install_is_kept() {
    let g = FakeGuest::new("1.18.30");
    let (ok, log) = g.run(Some("1.18.30"), true);
    assert!(ok, "log:\n{log}");
    assert!(log.contains("already installed: 1.18.30"), "log:\n{log}");
    assert_eq!(g.installed(), "1.18.30");
}

#[test]
fn stale_install_upgrades_via_npm() {
    let g = FakeGuest::new("1.0.0");
    let (ok, log) = g.run(Some("1.18.30"), true);
    assert!(ok, "log:\n{log}");
    assert!(
        log.contains("npm install -g opencode-ai@1.18.30"),
        "log:\n{log}"
    );
    assert!(log.contains("ready:"), "log:\n{log}");
    assert_eq!(g.installed(), "1.18.30");
}

#[test]
fn latest_resolves_via_npm() {
    let g = FakeGuest::new("0.0.0");
    // Empty machine: no version on record → npm installs latest.
    std::fs::remove_file(&g.version_file).unwrap();
    let (ok, log) = g.run(None, true);
    assert!(ok, "log:\n{log}");
    assert!(log.contains("opencode-ai@latest"), "log:\n{log}");
    assert_eq!(g.installed(), "9.9.9");
}

#[test]
fn pin_without_npm_fails_clearly() {
    let g = FakeGuest::new("1.0.0");
    let (ok, log) = g.run(Some("1.18.30"), false);
    assert!(!ok, "log:\n{log}");
    assert!(log.contains("cannot pin"), "log:\n{log}");
}
