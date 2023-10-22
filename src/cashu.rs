use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use cashu_sdk::client::Client;
use cashu_sdk::nuts::nut00::wallet::Token;
use cashu_sdk::nuts::nut03::RequestMintResponse;
use cashu_sdk::wallet::Wallet as CashuWallet;
use cashu_sdk::Amount;
use nostr_sdk::Url;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tracing::{debug, warn};

use crate::config::Settings;
use crate::database::Db;
use crate::error::Error;
use crate::nostr::Nostr;
use crate::types::{unix_time, PendingInvoice, UserKind};

#[derive(Debug, Clone)]
pub struct Cashu {
    mints: Arc<Mutex<HashMap<String, Option<CashuWallet>>>>,
    db: Db,
    nostr: Nostr,
    settings: Settings,
}

impl Cashu {
    pub fn new(db: Db, nostr: Nostr, settings: Settings) -> Self {
        Self {
            mints: Arc::new(Mutex::new(HashMap::new())),
            db,
            nostr,
            settings,
        }
    }

    /// Get wallet for uri
    async fn wallet_for_url(&self, mint_url: &nostr_sdk::Url) -> Result<CashuWallet, Error> {
        let mint_url = mint_url
            .as_str()
            .strip_suffix('/')
            .unwrap_or(mint_url.as_str());

        let mut wallets = self.mints.lock().await;
        let cashu_wallet = match wallets.get(mint_url) {
            Some(Some(wallet)) => wallet.clone(),
            _ => {
                let client = Client::new(mint_url)?;
                let keys = client.get_keys().await?;
                let wallet = CashuWallet::new(client, keys);
                wallets.insert(mint_url.to_string(), Some(wallet.clone()));

                wallet
            }
        };

        Ok(cashu_wallet)
    }

    pub async fn run(&self) -> Result<()> {
        loop {
            if let Err(err) = self.check_invoice().await {
                warn!("{}", err);
            }
        }
    }

    async fn check_invoice(&self) -> Result<()> {
        loop {
            let pending_invoices = self.db.get_pending_invoices().await?;
            for invoice in pending_invoices {
                let time_since_checked = match invoice.last_checked {
                    Some(time) => unix_time() - time,
                    None => 10000,
                };
                if time_since_checked.gt(&15) {
                    let cashu = self.clone();
                    match cashu.mint(&invoice).await {
                        Ok(token) => {
                            debug!("Invoice Paid: {:?}", invoice);
                            // DM token to nostr npub
                            let user = cashu.db.get_user(&invoice.username).await?;

                            if let Some(UserKind::User(user)) = user {
                                cashu
                                    .nostr
                                    .send_token(&user.pubkey, token, &user.relays)
                                    .await?;

                                if invoice.proxied && self.settings.info.zapper.unwrap_or(false) {
                                    if let Some(description) = invoice.description {
                                        if let Err(err) = self
                                            .nostr
                                            .broadcast_zap(
                                                invoice.bolt11,
                                                &description,
                                                &user.relays,
                                            )
                                            .await
                                        {
                                            warn!("Could not broadcast zap: {}", err);
                                        }
                                    }
                                }
                            }

                            // Remove token from pending
                            cashu.db.remove_pending_invoice(&invoice.hash).await?;
                        }
                        Err(err) => {
                            // Err or token is just unpaid
                            // Update checked time
                            warn!("{}", err);

                            let updated_invoice = invoice.update_checked_time();

                            cashu
                                .db
                                .add_pending_invoice(&invoice.hash, &updated_invoice)
                                .await?;
                        }
                    }
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn request_mint(
        &self,
        amount: Amount,
        mint_url: &Url,
    ) -> Result<RequestMintResponse, Error> {
        let wallet = self.wallet_for_url(mint_url).await?;
        debug!("Got wallet");
        let invoice = wallet.request_mint(amount).await?;

        Ok(invoice)
    }

    pub async fn mint(&self, pending_invoice: &PendingInvoice) -> Result<Token> {
        let wallet = self.wallet_for_url(&pending_invoice.mint).await?;

        Ok(wallet
            .mint_token(pending_invoice.amount, &pending_invoice.hash)
            .await?)
    }

    pub async fn add_pending_invoice(&self, pending_invoice: &PendingInvoice) -> Result<()> {
        self.db
            .add_pending_invoice(&pending_invoice.hash, pending_invoice)
            .await?;

        Ok(())
    }
}
