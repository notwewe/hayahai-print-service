use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tokio::sync::{Mutex, RwLock};

use crate::{
    api::ApiClient,
    model::{AgentConfig, AgentStatus},
    printer::{self, SpoolOutcome},
};

#[derive(Clone)]
pub struct AgentRuntime {
    pub config: Arc<Mutex<Option<AgentConfig>>>,
    pub signing_key: Arc<RwLock<SigningKey>>,
    pub status: Arc<RwLock<AgentStatus>>,
    pub active_job: Arc<AtomicBool>,
}

impl AgentRuntime {
    pub fn new(config: Option<AgentConfig>, signing_key: SigningKey) -> Self {
        let status = AgentStatus {
            paired: config.is_some(),
            api_url: config.as_ref().map(|value| value.api_url.clone()),
            agent_id: config.as_ref().map(|value| value.agent_id.clone()),
            printers: Vec::new(),
            active_job: false,
            last_error: None,
            version: env!("CARGO_PKG_VERSION").into(),
        };
        Self {
            config: Arc::new(Mutex::new(config)),
            signing_key: Arc::new(RwLock::new(signing_key)),
            status: Arc::new(RwLock::new(status)),
            active_job: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn set_error(&self, error: impl ToString) {
        self.status.write().await.last_error = Some(error.to_string());
    }
}

pub async fn run(app: AppHandle, runtime: AgentRuntime, api: ApiClient) {
    loop {
        if runtime.config.lock().await.is_none() {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let printers = match printer::discover().await {
            Ok(printers) => printers,
            Err(error) => {
                runtime.set_error(error).await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        {
            let mut status = runtime.status.write().await;
            status.printers = printers.clone();
            status.last_error = None;
        }
        if let Err(error) = api.heartbeat(&printers).await {
            runtime.set_error(error).await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }
        match api.claim().await {
            Ok(Some(job)) => {
                runtime.active_job.store(true, Ordering::SeqCst);
                runtime.status.write().await.active_job = true;
                let outcome =
                    process(&job.printer.queue_id, &job.id, &job.raw, &job.artifact_hash).await;
                let (state, error) = match outcome {
                    SpoolOutcome::Submitted => ("submitted", None),
                    SpoolOutcome::Failed(error) => ("failed", Some(error)),
                    SpoolOutcome::Unknown(error) => ("unknown", Some(error)),
                };
                if let Err(error) = api.acknowledge(&job.id, state, error.as_deref()).await {
                    runtime.set_error(error).await;
                }
                runtime.active_job.store(false, Ordering::SeqCst);
                runtime.status.write().await.active_job = false;
            }
            Ok(None) => {}
            Err(error) => {
                runtime.set_error(error).await;
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
        let _ = &app;
    }
}

async fn process(queue: &str, job_id: &str, raw: &str, expected_hash: &str) -> SpoolOutcome {
    let bytes = match STANDARD.decode(raw) {
        Ok(bytes) => bytes,
        Err(_) => return SpoolOutcome::Failed("The print artifact is not valid base64".into()),
    };
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected_hash {
        return SpoolOutcome::Failed("The print artifact hash did not match".into());
    }
    printer::spool(queue, job_id, &bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_changed_artifact_before_spooling() {
        let result = process("unused", "job", "AQID", "wrong").await;
        assert!(matches!(result, SpoolOutcome::Failed(_)));
    }
}
