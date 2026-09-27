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
    model::{AgentConfig, AgentStatus, ClaimedJob},
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
                log::info!(
                    "Claimed print job {} for queue {}",
                    job.id,
                    job.printer.queue_id
                );
                let outcome = process(&job).await;
                let (state, error) = match outcome {
                    SpoolOutcome::Submitted => {
                        log::info!("Print job {} completed in the OS queue", job.id);
                        ("submitted", None)
                    }
                    SpoolOutcome::Failed(error) => {
                        log::warn!("Print job {} failed safely: {}", job.id, error);
                        ("failed", Some(error))
                    }
                    SpoolOutcome::Unknown(error) => {
                        log::warn!("Print job {} has an uncertain outcome: {}", job.id, error);
                        ("unknown", Some(error))
                    }
                };
                if let Err(ack_error) = api.acknowledge(&job.id, state, error.as_deref()).await {
                    log::warn!("Could not acknowledge print job {}: {}", job.id, ack_error);
                    runtime.set_error(ack_error).await;
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

fn decode_artifact(encoded: &str, expected_hash: &str) -> Result<Vec<u8>, String> {
    let bytes = match STANDARD.decode(encoded) {
        Ok(bytes) => bytes,
        Err(_) => return Err("The print artifact is not valid base64".into()),
    };
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected_hash {
        return Err("The print artifact hash did not match".into());
    }
    Ok(bytes)
}

async fn process(job: &ClaimedJob) -> SpoolOutcome {
    #[cfg(target_os = "macos")]
    {
        let Some(options) = &job.print_options else {
            return SpoolOutcome::Failed(
                "This print job requires HayahAI Client API with macOS PDF printing support".into(),
            );
        };
        if let Some(documents) = job.documents.as_ref().filter(|pages| !pages.is_empty()) {
            let mut submitted_pages = 0usize;
            for (index, document) in documents.iter().enumerate() {
                let bytes = match decode_artifact(&document.pdf, &document.pdf_hash) {
                    Ok(bytes) => bytes,
                    Err(error) if submitted_pages == 0 => return SpoolOutcome::Failed(error),
                    Err(error) => {
                        return SpoolOutcome::Unknown(format!(
                            "{submitted_pages} page(s) printed before page {} failed validation: {error}",
                            index + 1
                        ));
                    }
                };
                let page_job_id = format!("{}-page-{}", job.id, index + 1);
                match printer::spool(
                    &job.printer.queue_id,
                    &page_job_id,
                    &bytes,
                    Some(options),
                    Some((document.width_mm, document.height_mm)),
                )
                .await
                {
                    SpoolOutcome::Submitted => submitted_pages += 1,
                    SpoolOutcome::Failed(error) if submitted_pages == 0 => {
                        return SpoolOutcome::Failed(error);
                    }
                    SpoolOutcome::Failed(error) | SpoolOutcome::Unknown(error) => {
                        return SpoolOutcome::Unknown(format!(
                            "{submitted_pages} page(s) printed before page {} had an uncertain result: {error}",
                            index + 1
                        ));
                    }
                }
            }
            return SpoolOutcome::Submitted;
        }
        let (Some(pdf), Some(pdf_hash)) = (&job.pdf, &job.pdf_hash) else {
            return SpoolOutcome::Failed("The macOS PDF artifact is missing".into());
        };
        let bytes = match decode_artifact(pdf, pdf_hash) {
            Ok(bytes) => bytes,
            Err(error) => return SpoolOutcome::Failed(error),
        };
        return printer::spool(&job.printer.queue_id, &job.id, &bytes, Some(options), None).await;
    }
    #[cfg(target_os = "windows")]
    {
        let bytes = match decode_artifact(&job.raw, &job.artifact_hash) {
            Ok(bytes) => bytes,
            Err(error) => return SpoolOutcome::Failed(error),
        };
        return printer::spool(&job.printer.queue_id, &job.id, &bytes, None, None).await;
    }
    #[allow(unreachable_code)]
    SpoolOutcome::Failed("Unsupported operating system".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_changed_artifact_before_spooling() {
        assert_eq!(
            decode_artifact("AQID", "wrong"),
            Err("The print artifact hash did not match".into())
        );
    }
}
