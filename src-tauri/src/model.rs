use serde::{Deserialize, Serialize};

pub const PROTOCOL: &str = "hayahai-print-agent-v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub api_url: String,
    pub agent_id: String,
    pub tenant_id: i32,
    pub protocol: String,
    pub counter: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterInfo {
    pub queue_id: String,
    pub display_name: String,
    pub driver_name: Option<String>,
    pub transport: String,
    pub capabilities: serde_json::Value,
    pub environment_hash: String,
    pub available: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollRequest {
    pub enrollment_id: String,
    pub secret: String,
    pub public_key: String,
    pub hostname: String,
    pub username: Option<String>,
    pub operating_system: String,
    pub architecture: String,
    pub version: String,
    pub protocol: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResponse {
    pub id: String,
    pub tenant_id: i32,
    pub protocol: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimedPrinter {
    pub queue_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimedJob {
    pub id: String,
    pub raw: String,
    pub artifact_hash: String,
    pub printer: ClaimedPrinter,
}

#[derive(Debug, Deserialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub paired: bool,
    pub api_url: Option<String>,
    pub agent_id: Option<String>,
    pub printers: Vec<PrinterInfo>,
    pub active_job: bool,
    pub last_error: Option<String>,
    pub version: String,
}
