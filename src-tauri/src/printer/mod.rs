use crate::{AgentError, model::PrinterInfo};

#[derive(Debug)]
pub enum SpoolOutcome {
    Submitted,
    Failed(String),
    Unknown(String),
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub async fn discover() -> Result<Vec<PrinterInfo>, AgentError> {
    #[cfg(target_os = "macos")]
    return macos::discover().await;
    #[cfg(target_os = "windows")]
    return windows::discover().await;
    #[allow(unreachable_code)]
    Err(AgentError::UnsupportedPlatform)
}

pub async fn spool(queue: &str, job_id: &str, bytes: &[u8]) -> SpoolOutcome {
    #[cfg(target_os = "macos")]
    return macos::spool(queue, job_id, bytes).await;
    #[cfg(target_os = "windows")]
    return windows::spool(queue, job_id, bytes).await;
    #[allow(unreachable_code)]
    SpoolOutcome::Failed("Unsupported operating system".into())
}
