use std::time::Duration;

use anyhow::{Context as _, Result};
use opencode_codes::client_async::OpencodeClient;
use opencode_codes::server::ManagedServer;

/// A locally managed `opencode serve` process plus a client bound to it.
pub struct AdeServer {
    pub managed: Option<ManagedServer>,
    pub client: OpencodeClient,
    pub base_url: String,
}

impl AdeServer {
    /// Spawn `opencode serve` in `project_dir` (unless `ADE_EXTERNAL_SERVER_URL`
    /// is set, in which case attach to that server instead) and build a client.
    pub async fn start(config: &crate::config::AppConfig) -> Result<Self> {
        if let Ok(url) = std::env::var("ADE_EXTERNAL_SERVER_URL") {
            tracing::info!("attaching to external opencode server at {url}");
            let client = OpencodeClient::builder()
                .base_url(&url)
                .auth_from_env()
                .timeout(Duration::from_secs(60))
                .build()?;
            return Ok(Self {
                managed: None,
                client,
                base_url: url.trim_end_matches('/').to_string(),
            });
        }

        let port = portpicker::pick_unused_port().context("no free TCP port for opencode serve")?;
        let managed = ManagedServer::builder()
            .binary(config.opencode_binary.clone())
            .hostname("127.0.0.1")
            .port(port)
            .working_dir(config.project_dir.clone())
            .startup_timeout(Duration::from_secs(30))
            .spawn()
            .await
            .context("spawning opencode serve")?;
        tracing::info!("opencode serve at {}", managed.url());
        let base_url = managed.url().trim_end_matches('/').to_string();

        let client = OpencodeClient::builder()
            .base_url(&base_url)
            .auth_from_env()
            .timeout(Duration::from_secs(60))
            .build()?;

        Ok(Self {
            managed: Some(managed),
            client,
            base_url,
        })
    }

    pub async fn health(&self) -> Result<bool> {
        #[derive(serde::Deserialize)]
        struct Health {
            #[serde(default)]
            healthy: bool,
        }
        let h: Health = self
            .client
            .request(reqwest::Method::GET, "/global/health", None)
            .await?;
        Ok(h.healthy)
    }

    pub async fn shutdown(mut self) {
        if let Some(m) = self.managed.take() {
            let _ = m.stop().await;
        }
    }
}
