//! End-to-end `ExternalSbxBackend` against a fake `sbx` shell script.
//! Pins the assumed CLI syntax (`create --name`, `ls` table, `rm --force`,
//! `setup ssh`) so the `ADE_LIVE_SBX=1` live test can confirm or correct it.

use std::io::Write as _;
use std::path::PathBuf;

use ade_vm::{ExternalSbxBackend, VmBackend, VmConfig, VmState};

struct FakeSbx {
    dir: PathBuf,
    bin: PathBuf,
}

impl FakeSbx {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("ade-fake-sbx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("sbx");
        let script = r#"#!/bin/sh
# FAKE sbx for ade-vm tests. State: $FAKE_SBX_DIR/<name>.status
cmd="$1"; shift
case "$cmd" in
  create)
    name=""; repo=""
    while [ $# -gt 0 ]; do
      case "$1" in --name) name="$2"; shift 2;; shell) shift;; *) repo="$1"; shift;; esac
    done
    echo "running" > "$FAKE_SBX_DIR/$name.status"
    echo "$repo" > "$FAKE_SBX_DIR/$name.repo"
    ;;
  ls)
    echo "SANDBOX AGENT STATUS PORTS WORKSPACE"
    for f in "$FAKE_SBX_DIR"/*.status; do
      [ -e "$f" ] || continue
      n=$(basename "$f" .status)
      echo "$n shell $(cat "$f") - Planner"
    done
    ;;
  rm) # rm --force <name>
    rm -f "$FAKE_SBX_DIR/$2.status"
    ;;
  setup) exit 0 ;; # setup ssh
  *) echo "unknown: $cmd" >&2; exit 1 ;;
esac
"#;
        let mut f = std::fs::File::create(&bin).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        unsafe {
            std::env::set_var("FAKE_SBX_DIR", dir.to_string_lossy().into_owned());
        }
        Self { dir, bin }
    }
}

impl Drop for FakeSbx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn shim_boot_ls_stop() {
    let fake = FakeSbx::new();
    let mut b = ExternalSbxBackend::with_bin(fake.bin.clone());
    let cfg = VmConfig {
        mount_repo: PathBuf::from("/repo/demo"),
        ..VmConfig::default()
    };
    let h = b.boot(&cfg).unwrap();
    assert_eq!(h.id, "ade-demo");
    // `ls` reports running.
    assert_eq!(b.state_of(&h), VmState::Running);
    b.stop(&h).unwrap();
    assert_eq!(b.state_of(&h), VmState::Stopped);
    assert!(b.stop(&h).is_err());
}
