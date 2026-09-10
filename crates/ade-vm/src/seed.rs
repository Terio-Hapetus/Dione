use std::path::Path;
use std::process::Command;

/// cloud-init NoCloud seed: injects the ephemeral pubkey, mounts the
/// virtiofs tag, and exposes sshd over vsock via socat.
/// Field names follow cloud-init schema; the live test (ADE_LIVE_VM=1)
/// is the final validator of every line below.
pub fn user_data(pubkey_openssh: &str, fs_tag: &str, guest_dir: &str, vsock_port: u32) -> String {
    format!(
        r#"#cloud-config
users:
  - name: ubuntu
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh-authorized-keys:
      - {pubkey_openssh}
packages:
  - socat
  - openssh-server
mounts:
  - [{fs_tag}, {guest_dir}, virtiofs, "defaults", "0", "0"]
runcmd:
  - [mkdir, -p, {guest_dir}]
  - [mount, -t, virtiofs, {fs_tag}, {guest_dir}]
  - [systemctl, enable, --now, ssh]
  - [sh, -c, "nohup socat VSOCK-LISTEN:{vsock_port},fork TCP:localhost:22 >/var/log/ade-socat.log 2>&1 &"]
"#
    )
}

pub fn meta_data(instance_id: &str) -> String {
    format!("instance-id: {instance_id}\nlocal-hostname: ade-vm\n")
}

/// Guest mount point for the shared repo (matches MicroVm Mounted mapping).
pub const GUEST_DIR: &str = "/workspace";
/// virtiofs tag: must equal the `fs.tag` in vm.create.
pub const FS_TAG: &str = "workspace";

/// Writes user-data + meta-data into `dir` and packs `seed.iso` with
/// `genisoimage` (live-machine tool). Returns the iso path.
pub fn build_seed_iso(
    dir: &Path,
    instance_id: &str,
    pubkey_openssh: &str,
    vsock_port: u32,
) -> anyhow::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join("user-data"),
        user_data(pubkey_openssh, FS_TAG, GUEST_DIR, vsock_port),
    )?;
    std::fs::write(dir.join("meta-data"), meta_data(instance_id))?;
    let iso = dir.join("seed.iso");
    let out = Command::new("genisoimage")
        .args([
            "-output",
            &iso.to_string_lossy(),
            "-volid",
            "cidata",
            "-joliet",
            "-rock",
            &dir.join("user-data").to_string_lossy(),
            &dir.join("meta-data").to_string_lossy(),
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("genisoimage missing: {e:#}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "genisoimage failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(iso)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_data_carries_key_mount_and_socat() {
        let ud = user_data("ssh-ed25519 AAAA test", "workspace", "/workspace", 2222);
        assert!(ud.contains("ssh-ed25519 AAAA test"));
        assert!(ud.contains("[workspace, /workspace, virtiofs"));
        assert!(ud.contains("VSOCK-LISTEN:2222,fork TCP:localhost:22"));
        assert!(ud.contains("openssh-server"));
    }

    #[test]
    fn meta_data_ids_instance() {
        let md = meta_data("ch-demo-3");
        assert!(md.contains("instance-id: ch-demo-3"));
        assert!(!md.contains("cidata"));
    }

    #[test]
    fn seed_iso_needs_genisoimage() {
        // No genisoimage here → clean error, not a panic. Live machine runs it.
        let dir = std::env::temp_dir().join("ade-seed-probe");
        let r = build_seed_iso(&dir, "x", "ssh-ed25519 AAAA", 2222);
        if which_genisoimage() {
            assert!(r.is_ok());
        } else {
            assert!(r.is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn which_genisoimage() -> bool {
        std::process::Command::new("genisoimage")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
