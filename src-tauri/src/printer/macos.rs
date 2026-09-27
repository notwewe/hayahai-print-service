use std::process::Stdio;
use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::time::{Instant, sleep};
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{
    AgentError,
    model::{ClaimedPrintOptions, PrinterInfo},
};

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
            capabilities: json!({ "pdf": true, "provider": "cups" }),
            environment_hash: fingerprint,
            available: !line.contains("disabled"),
        });
    }
    Ok(printers)
}

fn millimeters(value: f64) -> String {
    let value = format!("{value:.2}");
    value.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn driver_options(
    options: &ClaimedPrintOptions,
    page_size_mm: Option<(f64, f64)>,
) -> Result<Vec<String>, String> {
    if !matches!(options.paper_width_mm, 58 | 80) {
        return Err(format!(
            "Unsupported receipt paper width: {}mm",
            options.paper_width_mm
        ));
    }
    let page_size = if let Some((width, height)) = page_size_mm {
        if !(width.is_finite()
            && height.is_finite()
            && (25.4..=80.0).contains(&width)
            && (25.4..=2000.0).contains(&height))
        {
            return Err("The rendered receipt page size is invalid".into());
        }
        format!("Custom.{}x{}mm", millimeters(width), millimeters(height))
    } else {
        match options.paper_width_mm {
            58 => "RP58x2000".into(),
            80 => "RP80x2000".into(),
            _ => unreachable!(),
        }
    };
    let cut = match options.cut.as_str() {
        "none" => "TmxPaperCut=NoCut",
        "partial" | "full" => "TmxPaperCut=CutPerPage",
        value => return Err(format!("Unsupported receipt cutter mode: {value}")),
    };
    Ok(vec![
        format!("media={page_size}"),
        format!("PageSize={page_size}"),
        format!("Resolution={}x{}dpi", options.dpi, options.dpi),
        "sides=one-sided".into(),
        "TmxPaperReduction=Bottom".into(),
        cut.into(),
    ])
}

pub async fn spool(
    queue: &str,
    job_id: &str,
    bytes: &[u8],
    options: Option<&ClaimedPrintOptions>,
    page_size_mm: Option<(f64, f64)>,
) -> SpoolOutcome {
    let Some(options) = options else {
        return SpoolOutcome::Failed("The macOS printer settings are missing".into());
    };
    let options = match driver_options(options, page_size_mm) {
        Ok(options) => options,
        Err(error) => return SpoolOutcome::Failed(error),
    };
    let mut command = Command::new("/usr/bin/lp");
    command.args([
        "-d",
        queue,
        "-t",
        &format!("HayahAI-{job_id}"),
        "-o",
        "document-format=application/pdf",
    ]);
    for option in &options {
        command.args(["-o", option]);
    }
    let mut child = match command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
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
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let Some(cups_job_id) = parse_cups_job_id(&stdout) else {
                return SpoolOutcome::Unknown(
                    "CUPS accepted the job but did not return a job identifier. Check the printer before reprinting."
                        .into(),
                );
            };
            wait_for_completion(cups_job_id).await
        }
        Ok(output) => SpoolOutcome::Unknown(format!(
            "CUPS did not confirm submission: {}",
            concise(&String::from_utf8_lossy(&output.stderr))
        )),
        Err(error) => SpoolOutcome::Unknown(format!("CUPS outcome is unknown: {error}")),
    }
}

const CUPS_JOB_TEST: &str = "/usr/share/cups/ipptool/get-job-attributes.test";
const CUPS_POLL_INTERVAL: Duration = Duration::from_millis(500);
const CUPS_COMPLETION_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Default, PartialEq, Eq)]
struct CupsJobStatus {
    state: Option<String>,
    reasons: Vec<String>,
    message: Option<String>,
}

fn parse_cups_job_id(output: &str) -> Option<u64> {
    output.lines().find_map(|line| {
        let request = line.trim().strip_prefix("request id is ")?;
        request
            .split_whitespace()
            .next()?
            .rsplit_once('-')?
            .1
            .parse()
            .ok()
    })
}

fn parse_attribute(line: &str, name: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with(name) {
        return None;
    }
    let remainder = &line[name.len()..];
    if !remainder.starts_with(' ') && !remainder.starts_with('(') {
        return None;
    }
    remainder
        .split_once(" = ")
        .map(|(_, value)| value.trim().trim_matches('"').to_owned())
}

fn parse_cups_status(output: &str) -> CupsJobStatus {
    let mut status = CupsJobStatus::default();
    for line in output.lines() {
        if let Some(value) = parse_attribute(line, "job-state") {
            status.state = Some(value);
        } else if let Some(value) = parse_attribute(line, "job-state-reasons") {
            status.reasons.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && *value != "none")
                    .map(str::to_owned),
            );
        } else if let Some(value) = parse_attribute(line, "job-printer-state-message")
            && !value.is_empty()
        {
            status.message = Some(value);
        }
    }
    status
}

fn terminal_failure(status: &CupsJobStatus) -> bool {
    matches!(
        status.state.as_deref(),
        Some("aborted" | "canceled" | "processing-stopped")
    ) || status.reasons.iter().any(|reason| {
        let reason = reason.to_ascii_lowercase();
        reason.contains("stopped") || reason.contains("unable-to-send")
    }) || status.message.as_ref().is_some_and(|message| {
        let message = message.to_ascii_lowercase();
        message.contains("unable to send") || message.contains("printer stopped")
    })
}

fn concise(value: &str) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    single_line.chars().take(300).collect()
}

fn status_description(status: &CupsJobStatus) -> String {
    let state = status.state.as_deref().unwrap_or("unknown");
    let mut details = Vec::new();
    if !status.reasons.is_empty() {
        details.push(status.reasons.join(", "));
    }
    if let Some(message) = &status.message {
        details.push(concise(message));
    }
    if details.is_empty() {
        state.to_owned()
    } else {
        format!("{state}: {}", details.join("; "))
    }
}

async fn query_cups_job(job_id: u64) -> Result<CupsJobStatus, String> {
    let output = Command::new("/usr/bin/ipptool")
        .args([
            "-tv",
            &format!("ipp://localhost:631/jobs/{job_id}"),
            CUPS_JOB_TEST,
        ])
        .output()
        .await
        .map_err(|error| format!("Could not query CUPS job {job_id}: {error}"))?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(format!(
            "CUPS job {job_id} query failed: {}",
            concise(&combined)
        ));
    }
    let status = parse_cups_status(&combined);
    if status.state.is_none() {
        return Err(format!("CUPS job {job_id} returned no job state"));
    }
    Ok(status)
}

async fn wait_for_completion(job_id: u64) -> SpoolOutcome {
    let deadline = Instant::now() + CUPS_COMPLETION_TIMEOUT;
    loop {
        let detail = match query_cups_job(job_id).await {
            Ok(status) if status.state.as_deref() == Some("completed") => {
                return SpoolOutcome::Submitted;
            }
            Ok(status) if terminal_failure(&status) => {
                return SpoolOutcome::Unknown(format!(
                    "CUPS did not complete job {job_id} ({}). Check printer power, paper, cover, and USB connection before reprinting.",
                    status_description(&status)
                ));
            }
            Ok(status) => status_description(&status),
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return SpoolOutcome::Unknown(format!(
                "CUPS job {job_id} did not complete within 90 seconds ({detail}). Check the printer before reprinting."
            ));
        }
        sleep(CUPS_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_job_identifier_from_lp_response() {
        assert_eq!(
            parse_cups_job_id("request id is EPSON_TM_T82X_II-79 (0 file(s))\n"),
            Some(79)
        );
        assert_eq!(parse_cups_job_id("unrecognized response"), None);
    }

    #[test]
    fn parses_completed_ipp_job() {
        let status =
            parse_cups_status("job-state (enum) = completed\njob-state-reasons (keyword) = none\n");
        assert_eq!(status.state.as_deref(), Some("completed"));
        assert!(status.reasons.is_empty());
        assert!(!terminal_failure(&status));
    }

    #[test]
    fn detects_stopped_printer_and_delivery_error() {
        let status = parse_cups_status(
            "job-state (enum) = processing\njob-state-reasons (keyword) = printer-stopped\njob-printer-state-message (textWithoutLanguage) = Unable to send data to printer.\n",
        );
        assert!(terminal_failure(&status));
        assert_eq!(status.reasons, vec!["printer-stopped"]);
        assert_eq!(
            status.message.as_deref(),
            Some("Unable to send data to printer.")
        );
    }

    #[test]
    fn keeps_waiting_for_an_active_job() {
        let status = parse_cups_status(
            "job-state (enum) = processing\njob-state-reasons (keyword) = job-printing\njob-printer-state-message (textWithoutLanguage) = Sending data to printer.\n",
        );
        assert!(!terminal_failure(&status));
    }

    #[test]
    fn maps_managed_preset_to_epson_driver_options() {
        assert_eq!(
            driver_options(
                &ClaimedPrintOptions {
                    paper_width_mm: 80,
                    dpi: 203,
                    cut: "partial".into(),
                },
                Some((72.0, 145.0)),
            ),
            Ok(vec![
                "media=Custom.72x145mm".into(),
                "PageSize=Custom.72x145mm".into(),
                "Resolution=203x203dpi".into(),
                "sides=one-sided".into(),
                "TmxPaperReduction=Bottom".into(),
                "TmxPaperCut=CutPerPage".into(),
            ])
        );
        assert_eq!(
            driver_options(
                &ClaimedPrintOptions {
                    paper_width_mm: 58,
                    dpi: 300,
                    cut: "none".into(),
                },
                Some((48.01, 201.25)),
            ),
            Ok(vec![
                "media=Custom.48.01x201.25mm".into(),
                "PageSize=Custom.48.01x201.25mm".into(),
                "Resolution=300x300dpi".into(),
                "sides=one-sided".into(),
                "TmxPaperReduction=Bottom".into(),
                "TmxPaperCut=NoCut".into(),
            ])
        );
    }

    #[test]
    fn rejects_invalid_custom_roll_dimensions() {
        let options = ClaimedPrintOptions {
            paper_width_mm: 80,
            dpi: 203,
            cut: "partial".into(),
        };
        assert!(driver_options(&options, Some((72.0, 2000.1))).is_err());
        assert!(driver_options(&options, Some((f64::NAN, 145.0))).is_err());
    }
}
