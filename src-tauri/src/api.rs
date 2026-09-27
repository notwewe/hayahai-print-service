use std::sync::Arc;

use ed25519_dalek::SigningKey;
use reqwest::{Client, Method};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tauri::AppHandle;
use tokio::sync::{Mutex, RwLock};

use crate::{
    AgentError,
    model::{AgentConfig, ApiEnvelope, ClaimedJob, EnrollRequest, EnrollResponse, PrinterInfo},
    protocol::{signature, timestamp_ms},
    storage,
};

#[derive(Clone)]
pub struct ApiClient {
    app: AppHandle,
    http: Client,
    key: Arc<RwLock<SigningKey>>,
    config: Arc<Mutex<Option<AgentConfig>>>,
}

impl ApiClient {
    pub fn new(
        app: AppHandle,
        key: Arc<RwLock<SigningKey>>,
        config: Arc<Mutex<Option<AgentConfig>>>,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            app,
            key,
            config,
            http: Client::builder().https_only(false).build()?,
        })
    }

    pub async fn enroll(
        api_url: &str,
        request: &EnrollRequest,
    ) -> Result<EnrollResponse, AgentError> {
        validate_api_url(api_url)?;
        let response = Client::new()
            .post(format!(
                "{}/printing/agent/enroll",
                api_url.trim_end_matches('/')
            ))
            .json(request)
            .send()
            .await?;
        decode(response).await
    }

    async fn signed<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: Value,
    ) -> Result<T, AgentError> {
        // Keep the lock through the response so signed counters reach the API in order.
        let mut guard = self.config.lock().await;
        let config = guard.as_mut().ok_or_else(|| AgentError::NotPaired)?;
        config.counter = config
            .counter
            .checked_add(1)
            .ok_or_else(|| AgentError::Security("Request counter exhausted".into()))?;
        storage::save_config(&self.app, config)?;
        let timestamp = timestamp_ms();
        let counter = config.counter;
        let signed = signature(
            &*self.key.read().await,
            method.as_str(),
            path,
            timestamp,
            counter,
            &body,
        );
        let mut url = format!("{}{}", config.api_url.trim_end_matches('/'), path);
        if let Some(query) = query {
            url.push('?');
            url.push_str(query);
        }
        let response = self
            .http
            .request(method, url)
            .header("x-print-agent-id", &config.agent_id)
            .header("x-print-agent-timestamp", timestamp.to_string())
            .header("x-print-agent-counter", counter.to_string())
            .header("x-print-agent-signature", signed)
            .json(&body)
            .send()
            .await?;
        decode(response).await
    }

    pub async fn heartbeat(&self, printers: &[PrinterInfo]) -> Result<(), AgentError> {
        let body = serde_json::json!({
            "printers": printers,
            "version": env!("CARGO_PKG_VERSION")
        });
        let _: Value = self
            .signed(Method::POST, "/printing/agent/heartbeat", None, body)
            .await?;
        Ok(())
    }

    pub async fn claim(&self) -> Result<Option<ClaimedJob>, AgentError> {
        self.signed(
            Method::POST,
            "/printing/agent/jobs/claim",
            Some("wait=25"),
            serde_json::json!({}),
        )
        .await
    }

    pub async fn acknowledge(
        &self,
        id: &str,
        state: &str,
        error: Option<&str>,
    ) -> Result<(), AgentError> {
        let body = serde_json::json!({ "state": state, "error": error });
        let _: Value = self
            .signed(
                Method::POST,
                &format!("/printing/agent/jobs/{id}/ack"),
                None,
                body,
            )
            .await?;
        Ok(())
    }
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, AgentError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AgentError::Api(api_error_message(status, &body)));
    }
    Ok(response.json::<ApiEnvelope<T>>().await?.data)
}

fn api_error_message(status: reqwest::StatusCode, body: &str) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return "This workstation pairing is no longer authorized. Generate a new pairing link in TMS, then select Pair a new link.".into();
    }
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("Request failed")
                .to_owned()
        });
    format!("Request failed ({status}): {message}")
}

pub fn validate_api_url(value: &str) -> Result<(), AgentError> {
    let parsed = url::Url::parse(value).map_err(|_| AgentError::InvalidPairing)?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if parsed.scheme() != "https" && !(cfg!(debug_assertions) && parsed.scheme() == "http" && local)
    {
        return Err(AgentError::Security(
            "Production pairing requires an HTTPS Client API endpoint".into(),
        ));
    }
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AgentError::InvalidPairing);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_errors_request_a_new_pairing_without_exposing_server_details() {
        let body = r#"{"message":"Invalid print-agent signature.","stack":"sensitive stack"}"#;
        let message = api_error_message(reqwest::StatusCode::UNAUTHORIZED, body);
        assert!(message.contains("Generate a new pairing link"));
        assert!(!message.contains("sensitive stack"));
    }

    #[test]
    fn other_api_errors_include_only_the_public_message() {
        let body = r#"{"message":"Printer is unavailable.","stack":"sensitive stack"}"#;
        let message = api_error_message(reqwest::StatusCode::BAD_REQUEST, body);
        assert!(message.contains("Printer is unavailable."));
        assert!(!message.contains("sensitive stack"));
    }
}
