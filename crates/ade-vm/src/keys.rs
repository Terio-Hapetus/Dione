use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_KEY_DIR: AtomicU64 = AtomicU64::new(1);

/// Ephemeral SSH keypair: generated fresh per boot, private key lives in
/// a temp dir that is wiped on drop. Never reused, never copied to the VM
/// (only the public half is injected via cloud-init / sbx setup).
#[derive(Debug)]
pub struct EphemeralKey {
    pubkey_openssh: String,
    priv_path: PathBuf,
    dir: PathBuf,
}

impl EphemeralKey {
    pub fn generate() -> anyhow::Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "ade-key-{}-{}",
            std::process::id(),
            NEXT_KEY_DIR.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| anyhow::anyhow!("key dir failed: {e:#}"))?;
        // Lock the dir down before the key lands in it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let priv_path = dir.join("id_ed25519");
        let out = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "ade-ephemeral",
                "-f",
                &priv_path.to_string_lossy(),
            ])
            .output()
            .map_err(|e| anyhow::anyhow!("ssh-keygen missing or failed: {e:#}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "ssh-keygen failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let pubkey_openssh = std::fs::read_to_string(dir.join("id_ed25519.pub"))?
            .trim()
            .to_string();
        Ok(Self {
            pubkey_openssh,
            priv_path,
            dir,
        })
    }

    /// `ssh -i` argument.
    pub fn priv_path(&self) -> &Path {
        &self.priv_path
    }

    /// Single-line `authorized_keys` / cloud-init entry.
    pub fn pubkey_openssh(&self) -> &str {
        &self.pubkey_openssh
    }
}

impl Drop for EphemeralKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_ed25519_pair() {
        let k = EphemeralKey::generate().unwrap();
        assert!(k.pubkey_openssh().starts_with("ssh-ed25519 "));
        assert!(k.priv_path().exists());
        // Private half is not world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(k.priv_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn drop_wipes_dir() {
        let dir = {
            let k = EphemeralKey::generate().unwrap();
            k.dir.clone()
        };
        assert!(!dir.exists());
    }

    #[test]
    fn each_key_unique() {
        let a = EphemeralKey::generate().unwrap();
        let b = EphemeralKey::generate().unwrap();
        assert_ne!(a.pubkey_openssh(), b.pubkey_openssh());
    }
}
