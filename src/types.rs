use std::time::SystemTime;

use cashu_crab::{Amount, Bolt11Invoice};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::utils::{amount_from_msat, msat_from_amount};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSignUp {
    /// Cashu mint
    pub mint: String,
    /// LN Address username
    pub username: String,
    /// Nostr Relays
    pub relays: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// Cashu mint
    pub mint: String,
    /// Nostr Pubkey
    pub pubkey: String,
    /// Nostr Relays
    pub relays: Vec<String>,
    /// Proxy Invoice to mint
    pub proxy: bool,
}

impl User {
    /// Get transaction as json string
    pub fn as_json(&self) -> String {
        serde_json::json!(self).to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInvoice {
    pub mint: String,
    pub username: String,
    pub description: Option<String>,
    pub time: u64,
    #[serde(with = "as_msat")]
    pub amount: Amount,
    pub hash: String,
    pub bolt11: Bolt11Invoice,
    pub last_checked: Option<u64>,
    pub proxied: bool,
}

impl PendingInvoice {
    /// Get transaction as json string
    pub fn as_json(&self) -> String {
        serde_json::json!(self).to_string()
    }

    pub fn update_checked_time(&self) -> Self {
        Self {
            mint: self.mint.clone(),
            username: self.username.clone(),
            description: self.description.clone(),
            time: self.time,
            amount: self.amount,
            hash: self.hash.clone(),
            bolt11: self.bolt11.clone(),
            last_checked: Some(unix_time()),
            proxied: self.proxied,
        }
    }
}

pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|x| x.as_secs())
        .unwrap_or(0)
}

pub mod as_msat {
    use super::*;

    pub fn serialize<S>(amount: &Amount, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let msats = msat_from_amount(amount);
        msats.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Amount, D::Error>
    where
        D: Deserializer<'de>,
    {
        let msat = u64::deserialize(deserializer)?;
        Ok(amount_from_msat(msat))
    }
}
