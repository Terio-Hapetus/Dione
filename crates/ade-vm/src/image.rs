use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned VM asset (kernel, cloud image): download once, verify, cache.
/// `sha256` is `"skip"` only in tests — production pins always verify.
#[derive(Debug, Clone)]
pub struct ImageSpec {
    pub url: String,
    pub sha256: String,
    pub filename: String,
}

impl ImageSpec {
    pub fn cached_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(&self.filename)
    }

    /// True when the cached file exists and its checksum matches.
    pub fn is_cached(&self, cache_dir: &Path) -> bool {
        let p = self.cached_path(cache_dir);
        p.exists() && verify_sha256(&p, &self.sha256).unwrap_or(false)
    }
}

/// Default cache: `~/.local/share/ade/vm/`.
pub fn default_cache_dir() -> PathBuf {
    dirs_fallback()
        .join(".local")
        .join("share")
        .join("ade")
        .join("vm")
}

fn dirs_fallback() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Fetch `spec` into `cache_dir` unless already cached+verified.
/// Uses the `curl` CLI (retry + resume) to avoid new HTTP deps.
pub fn ensure_image(spec: &ImageSpec, cache_dir: &Path) -> anyhow::Result<PathBuf> {
    let dest = spec.cached_path(cache_dir);
    if spec.is_cached(cache_dir) {
        return Ok(dest);
    }
    std::fs::create_dir_all(cache_dir)?;
    let tmp = cache_dir.join(format!("{}.part", spec.filename));
    let out = Command::new("curl")
        .args([
            "-fSL",
            "--retry",
            "3",
            "-C",
            "-",
            "-o",
            &tmp.to_string_lossy(),
            &spec.url,
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("curl missing or failed: {e:#}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "download {} failed: {}",
            spec.url,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if !verify_sha256(&tmp, &spec.sha256)? {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("checksum mismatch for {}", spec.filename);
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

pub fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<bool> {
    if expected == "skip" {
        return Ok(true);
    }
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| anyhow::anyhow!("sha256sum missing: {e:#}"))?;
    if !out.status.success() {
        anyhow::bail!("sha256sum failed for {}", path.display());
    }
    let actual = String::from_utf8_lossy(&out.stdout);
    let actual = actual.split_whitespace().next().unwrap_or("");
    Ok(actual.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn tmp_cache(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ade-img-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sha_of(data: &[u8]) -> String {
        let p = std::env::temp_dir().join(format!("ade-sha-{}.bin", std::process::id()));
        std::fs::write(&p, data).unwrap();
        let o = Command::new("sha256sum").arg(&p).output().unwrap();
        let _ = std::fs::remove_file(&p);
        String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    }

    #[test]
    fn file_url_download_and_cache_hit() {
        let src_dir = tmp_cache("src");
        let src = src_dir.join("asset.bin");
        let mut f = std::fs::File::create(&src).unwrap();
        f.write_all(b"fake-kernel-bytes").unwrap();
        let spec = ImageSpec {
            url: format!("file://{}", src.display()),
            sha256: sha_of(b"fake-kernel-bytes"),
            filename: "asset.bin".into(),
        };
        let cache = tmp_cache("cache");
        let p1 = ensure_image(&spec, &cache).unwrap();
        assert_eq!(std::fs::read(&p1).unwrap(), b"fake-kernel-bytes");
        // Second call: cache hit, no re-download (remove source to prove it).
        std::fs::remove_file(&src).unwrap();
        let p2 = ensure_image(&spec, &cache).unwrap();
        assert_eq!(p1, p2);
        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn checksum_mismatch_errors() {
        let src_dir = tmp_cache("src2");
        let src = src_dir.join("bad.bin");
        std::fs::write(&src, b"tampered").unwrap();
        let spec = ImageSpec {
            url: format!("file://{}", src.display()),
            sha256: "0".repeat(64),
            filename: "bad.bin".into(),
        };
        let cache = tmp_cache("cache2");
        assert!(ensure_image(&spec, &cache).is_err());
        // Corrupt file must not be left behind as cached.
        assert!(!cache.join("bad.bin").exists());
        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&src_dir);
    }
}
