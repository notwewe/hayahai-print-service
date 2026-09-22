use std::{fs, path::PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use tauri::{AppHandle, Manager};

use crate::{AgentError, model::AgentConfig};

const KEYRING_SERVICE: &str = "com.hayahai.printservice";
const KEYRING_USER: &str = "device-ed25519";

fn config_path(app: &AppHandle) -> Result<PathBuf, AgentError> {
    let directory = app.path().app_config_dir()?;
    fs::create_dir_all(&directory)?;
    Ok(directory.join("agent.json"))
}

pub fn load_config(app: &AppHandle) -> Result<Option<AgentConfig>, AgentError> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

pub fn save_config(app: &AppHandle, config: &AgentConfig) -> Result<(), AgentError> {
    let path = config_path(app)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(config)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn signing_key() -> Result<SigningKey, AgentError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?;
    match entry.get_password() {
        Ok(encoded) => {
            let bytes = STANDARD.decode(encoded)?;
            let key: [u8; 32] = bytes
                .try_into()
                .map_err(|_| AgentError::Security("Stored device key is invalid".into()))?;
            Ok(SigningKey::from_bytes(&key))
        }
        Err(keyring::Error::NoEntry) => {
            let key = new_signing_key();
            save_signing_key(&key)?;
            Ok(key)
        }
        Err(error) => Err(error.into()),
    }
}

pub fn new_signing_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

pub fn save_signing_key(key: &SigningKey) -> Result<(), AgentError> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?
        .set_password(&STANDARD.encode(key.to_bytes()))?;
    Ok(())
}
