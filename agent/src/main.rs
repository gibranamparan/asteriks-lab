use anyhow::{anyhow, Context, Result};
use futures_util::stream::StreamExt;
use lapin::{options::*, types::FieldTable, Connection, ConnectionProperties, ExchangeKind};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tokio::signal;
use tokio::time::{interval, Instant};
use tracing::{error, info, warn};

const DEBOUNCE_SECONDS: u64 = 10;
const FULL_SYNC_INTERVAL_SECONDS: u64 = 3600;
const PJSIP_FILE_PATH: &str = "/config/pjsip.conf";
const PJSIP_LAST_GOOD_PATH: &str = "/config/pjsip.conf.last-good";
const PJSIP_BACKUP_DIR: &str = "/config/backups";
const SHARED_PASSWORD: &str = "Sentrics2026";
const ASTERISK_CONTAINER: &str = "asterisk-server";

#[derive(Debug, Clone)]
struct AppConfig {
    headend_url: String,
    amqp_exchange: String,
    amqp_exchange_type: String,
    amqp_queue: String,
    amqp_routing_key: String,
}

#[derive(Debug, Deserialize)]
struct QueueEvent {
    id: String,
    event: String,
    resource: String,
}

#[derive(Debug, Deserialize)]
struct GqlResponse {
    data: GqlData,
}

#[derive(Debug, Deserialize)]
struct GqlData {
    intercoms: Vec<Intercom>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Intercom {
    id: String,
    mac: String,
}

impl AppConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            headend_url: required_env("HEADEND_URL")?,
            amqp_exchange: required_env("HEADEND_AMQP_EXCHANGE")?,
            amqp_exchange_type: env::var("HEADEND_AMQP_EXCHANGE_TYPE")
                .unwrap_or_else(|_| "topic".to_string()),
            amqp_queue: required_env("HEADEND_AMQP_QUEUE")?,
            amqp_routing_key: env::var("HEADEND_AMQP_ROUTING_KEY")
                .unwrap_or_else(|_| "#".to_string()),
        })
    }

    fn amqp_url(&self) -> String {
        format!("amqp://{}:5672", self.headend_url)
    }

    fn graphql_url(&self) -> String {
        format!("http://{}:5000/graphql", self.headend_url)
    }

    fn exchange_kind(&self) -> ExchangeKind {
        match self.amqp_exchange_type.to_ascii_lowercase().as_str() {
            "direct" => ExchangeKind::Direct,
            "fanout" => ExchangeKind::Fanout,
            "headers" => ExchangeKind::Headers,
            "topic" => ExchangeKind::Topic,
            other => {
                warn!(kind = other, "Unknown exchange type. Falling back to topic");
                ExchangeKind::Topic
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = match AppConfig::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            error!(error = %err, "Invalid or missing environment configuration. Agent cannot start");
            return Err(err);
        }
    };
    ensure_paths()?;

    info!(
        amqp_url = %cfg.amqp_url(),
        graphql_url = %cfg.graphql_url(),
        "Starting intercom sync agent"
    );

    // Startup full sync to reconcile state.
    if let Err(err) = run_sync(&cfg).await {
        warn!(error = %err, "Startup sync failed");
    }

    let conn = Connection::connect(&cfg.amqp_url(), ConnectionProperties::default())
        .await
        .context("Failed to connect to AMQP")?;
    let channel = conn
        .create_channel()
        .await
        .context("Failed to create AMQP channel")?;

    channel
        .exchange_declare(
            &cfg.amqp_exchange,
            cfg.exchange_kind(),
            ExchangeDeclareOptions::default(),
            FieldTable::default(),
        )
        .await
        .context("Failed to declare exchange")?;

    channel
        .queue_declare(
            &cfg.amqp_queue,
            QueueDeclareOptions::default(),
            FieldTable::default(),
        )
        .await
        .context("Failed to declare queue")?;

    channel
        .queue_bind(
            &cfg.amqp_queue,
            &cfg.amqp_exchange,
            &cfg.amqp_routing_key,
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await
        .context("Failed to bind queue")?;

    let mut consumer = channel
        .basic_consume(
            &cfg.amqp_queue,
            "intercom-sync-agent",
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await
        .context("Failed to consume queue")?;

    let mut debounce_deadline: Option<Instant> = None;
    let mut dirty = false;
    let mut full_sync_ticker = interval(Duration::from_secs(FULL_SYNC_INTERVAL_SECONDS));

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("Shutdown signal received");
                break;
            }
            _ = full_sync_ticker.tick() => {
                info!("Periodic full sync tick");
                if let Err(err) = run_sync(&cfg).await {
                    error!(error = %err, "Periodic full sync failed");
                }
            }
            maybe_delivery = consumer.next() => {
                match maybe_delivery {
                    Some(Ok(delivery)) => {
                        match handle_delivery(&delivery.data) {
                            Ok(should_schedule_sync) => {
                                if should_schedule_sync {
                                    dirty = true;
                                    debounce_deadline = Some(Instant::now() + Duration::from_secs(DEBOUNCE_SECONDS));
                                }
                            }
                            Err(err) => {
                                warn!(error = %err, "Dropping malformed event");
                            }
                        }

                        delivery
                            .ack(BasicAckOptions::default())
                            .await
                            .context("Failed to ack message")?;
                    }
                    Some(Err(err)) => {
                        warn!(error = %err, "AMQP delivery error");
                    }
                    None => {
                        return Err(anyhow!("AMQP consumer closed"));
                    }
                }
            }
            _ = async {
                if let Some(deadline) = debounce_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }, if debounce_deadline.is_some() => {
                if dirty {
                    info!("Debounce window elapsed. Running sync");
                    if let Err(err) = run_sync(&cfg).await {
                        error!(error = %err, "Debounced sync failed");
                    }
                }
                dirty = false;
                debounce_deadline = None;
            }
        }
    }

    Ok(())
}

fn handle_delivery(body: &[u8]) -> Result<bool> {
    let event: QueueEvent = serde_json::from_slice(body).context("Invalid event JSON")?;

    if is_relevant_event(&event) {
        info!(id = %event.id, event = %event.event, resource = %event.resource, "Relevant intercom event received");
        Ok(true)
    } else {
        info!(id = %event.id, event = %event.event, resource = %event.resource, "Ignoring non-relevant event");
        Ok(false)
    }
}

fn is_relevant_event(event: &QueueEvent) -> bool {
    if event.resource != "intercom" {
        return false;
    }

    matches!(event.event.as_str(), "create" | "update" | "delete")
}

async fn run_sync(cfg: &AppConfig) -> Result<()> {
    let intercoms = fetch_intercoms(cfg).await?;
    let rendered = render_pjsip_conf(&intercoms);

    if !should_apply(&rendered)? {
        info!("Generated pjsip.conf is unchanged. Skipping apply");
        return Ok(());
    }

    apply_config(&rendered)?;

    if let Err(err) = apply_to_asterisk().await {
        error!(error = %err, "pjsip reload failed. Restoring last good config and restarting container");
        restore_last_good()?;
        restart_asterisk_container().await?;
    }

    info!(count = intercoms.len(), "Sync applied successfully");
    Ok(())
}

async fn fetch_intercoms(cfg: &AppConfig) -> Result<Vec<Intercom>> {
    let client = Client::new();
    let response = client
        .post(cfg.graphql_url())
        .json(&json!({
            "query": "{ intercoms { id mac } }"
        }))
        .send()
        .await
        .context("Failed GraphQL request")?
        .error_for_status()
        .context("GraphQL returned non-success status")?;

    let payload: GqlResponse = response
        .json()
        .await
        .context("Failed to decode GraphQL response")?;

    let mut normalized = Vec::new();
    let mut dropped = 0usize;

    for i in payload.data.intercoms {
        match normalize_mac(&i.mac) {
            Some(mac) => normalized.push(Intercom { id: i.id, mac }),
            None => dropped += 1,
        }
    }

    normalized.sort_by(|a, b| a.mac.cmp(&b.mac));

    if dropped > 0 {
        warn!(dropped, "Dropped intercoms with invalid MAC");
    }

    Ok(normalized)
}

fn render_pjsip_conf(intercoms: &[Intercom]) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "; Generated by intercom-sync-agent at unix={}\n",
        unix_timestamp_secs()
    ));
    out.push_str(&format!("; Intercom count: {}\n\n", intercoms.len()));

    out.push_str("[transport-udp]\n");
    out.push_str("type=transport\n");
    out.push_str("protocol=udp\n");
    out.push_str("bind=0.0.0.0:5060\n\n");

    out.push_str("[endpoint-template](!)\n");
    out.push_str("type=endpoint\n");
    out.push_str("context=internal\n");
    out.push_str("disallow=all\n");
    out.push_str("allow=ulaw\n");
    out.push_str("allow=alaw\n\n");

    out.push_str("[auth-template](!)\n");
    out.push_str("type=auth\n");
    out.push_str("auth_type=userpass\n\n");

    out.push_str("[aor-template](!)\n");
    out.push_str("type=aor\n");
    out.push_str("max_contacts=1\n\n");

    for i in intercoms {
        let ext = &i.mac;
        out.push_str(&format!("; Intercom {}\n", i.id));
        out.push_str(&format!("[{}](endpoint-template)\n", ext));
        out.push_str(&format!("auth={}-auth\n", ext));
        out.push_str(&format!("aors={}\n\n", ext));

        out.push_str(&format!("[{}-auth](auth-template)\n", ext));
        out.push_str(&format!("username={}\n", ext));
        out.push_str(&format!("password={}\n\n", SHARED_PASSWORD));

        out.push_str(&format!("[{}](aor-template)\n\n", ext));
    }

    out
}

fn apply_config(content: &str) -> Result<()> {
    let pjsip_path = Path::new(PJSIP_FILE_PATH);
    let backup_dir = Path::new(PJSIP_BACKUP_DIR);
    let last_good_path = Path::new(PJSIP_LAST_GOOD_PATH);

    if pjsip_path.exists() {
        let ts = unix_timestamp_secs();
        let backup_file = backup_dir.join(format!("pjsip.conf.{ts}.bak"));
        fs::copy(pjsip_path, &backup_file)
            .with_context(|| format!("Failed backup copy to {}", backup_file.display()))?;
        fs::copy(pjsip_path, last_good_path)
            .with_context(|| format!("Failed to refresh {}", last_good_path.display()))?;
    }

    let temp_path = tmp_path_in_same_dir(pjsip_path)?;
    fs::write(&temp_path, content)
        .with_context(|| format!("Failed writing {}", temp_path.display()))?;
    fs::rename(&temp_path, pjsip_path)
        .with_context(|| format!("Failed swapping {}", pjsip_path.display()))?;

    cleanup_old_backups(backup_dir, 20)?;
    Ok(())
}

fn restore_last_good() -> Result<()> {
    let src = Path::new(PJSIP_LAST_GOOD_PATH);
    let dst = Path::new(PJSIP_FILE_PATH);

    if !src.exists() {
        return Err(anyhow!("No last-good pjsip file found"));
    }

    fs::copy(src, dst).with_context(|| {
        format!(
            "Failed restoring last-good config from {} to {}",
            src.display(),
            dst.display()
        )
    })?;

    Ok(())
}

async fn apply_to_asterisk() -> Result<()> {
    let output = Command::new("docker")
        .args([
            "exec",
            ASTERISK_CONTAINER,
            "asterisk",
            "-rx",
            "pjsip reload",
        ])
        .output()
        .await
        .context("Failed to execute pjsip reload")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("pjsip reload command failed: {stderr}"));
    }

    Ok(())
}

async fn restart_asterisk_container() -> Result<()> {
    let output = Command::new("docker")
        .args(["restart", ASTERISK_CONTAINER])
        .output()
        .await
        .context("Failed to execute docker restart")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("docker restart failed: {stderr}"));
    }

    Ok(())
}

fn ensure_paths() -> Result<()> {
    let pjsip = Path::new(PJSIP_FILE_PATH);
    let config_dir = pjsip
        .parent()
        .ok_or_else(|| anyhow!("Invalid pjsip file path"))?;

    fs::create_dir_all(config_dir)
        .with_context(|| format!("Failed to create config dir {}", config_dir.display()))?;

    fs::create_dir_all(PJSIP_BACKUP_DIR)
        .with_context(|| format!("Failed to create backup dir {}", PJSIP_BACKUP_DIR))?;

    Ok(())
}

fn tmp_path_in_same_dir(target: &Path) -> Result<PathBuf> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("Target path has no parent"))?;

    let filename = target
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| anyhow!("Invalid target filename"))?;

    Ok(parent.join(format!(".{filename}.tmp")))
}

fn should_apply(new_content: &str) -> Result<bool> {
    let path = Path::new(PJSIP_FILE_PATH);
    if !path.exists() {
        return Ok(true);
    }

    let existing =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;

    Ok(hash_str(&existing) != hash_str(new_content))
}

fn hash_str(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn cleanup_old_backups(dir: &Path, keep: usize) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read backup dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("pjsip.conf."))
        .collect::<Vec<_>>();

    entries.sort_by_key(|e| e.file_name());

    let remove_count = entries.len().saturating_sub(keep);
    for entry in entries.into_iter().take(remove_count) {
        let path = entry.path();
        if let Err(err) = fs::remove_file(&path) {
            warn!(path = %path.display(), error = %err, "Failed deleting old backup");
        }
    }

    Ok(())
}

fn normalize_mac(mac: &str) -> Option<String> {
    let cleaned = mac.trim().to_ascii_lowercase().replace(':', "");

    if cleaned.len() != 12 || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    Some(cleaned)
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("Missing required env var {name}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!("Environment variable {name} is empty"));
    }
    Ok(value)
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
