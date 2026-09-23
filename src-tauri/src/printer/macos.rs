use std::process::Stdio;

use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{AgentError, model::PrinterInfo};

use super::SpoolOutcome;

pub async fn discover() -> Result<Vec<PrinterInfo>, AgentError> {
    let output = Command::new("/usr/bin/lpstat").arg("-p").output().await?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.to_ascii_lowercase().contains("no destinations added") {
            return Ok(Vec::new());
        }
        return Err(AgentError::Printer("CUPS printer discovery failed".into()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut printers = Vec::new();
    for line in text.lines().filter(|line| line.starts_with("printer ")) {
        let Some(queue) = line.split_whitespace().nth(1) else {
            continue;
        };
        let details = Command::new("/usr/bin/lpoptions")
            .args(["-p", queue, "-l"])
            .output()
            .await?;
        let details_text = String::from_utf8_lossy(&details.stdout);
        let fingerprint = hex::encode(Sha256::digest(
            format!("{queue}\n{details_text}").as_bytes(),
        ));
        printers.push(PrinterInfo {
            queue_id: queue.into(),
            display_name: queue.into(),
            driver_name: details_text
                .lines()
                .next()
                .map(|value| value.chars().take(255).collect()),
            transport: "os-queue".into(),
            capabilities: json!({ "raw": true, "provider": "cups" }),
            environment_hash: fingerprint,
            available: !line.contains("disabled"),
        });
    }
    Ok(printers)
}

pub async fn spool(queue: &str, job_id: &str, bytes: &[u8]) -> SpoolOutcome {
    let mut child = match Command::new("/usr/bin/lp")
        .args(["-d", queue, "-o", "raw", "-t", &format!("HayahAI-{job_id}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return SpoolOutcome::Failed(format!("Could not start CUPS: {error}")),
    };
    let Some(mut input) = child.stdin.take() else {
        return SpoolOutcome::Failed("CUPS input was unavailable".into());
    };
    if let Err(error) = input.write_all(bytes).await {
        return SpoolOutcome::Unknown(format!("CUPS input write was interrupted: {error}"));
    }
    drop(input);
    match child.wait_with_output().await {
        Ok(output) if output.status.success() => SpoolOutcome::Submitted,
        Ok(output) => SpoolOutcome::Unknown(format!(
            "CUPS did not confirm submission: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => SpoolOutcome::Unknown(format!("CUPS outcome is unknown: {error}")),
    }
}
