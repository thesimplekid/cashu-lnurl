use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use cashu_crab::{
    cashu_wallet::CashuWallet,
    client::Client,
    types::{RequestMintResponse, Token},
    Amount,
};
use log::{debug, warn};
use tokio::{
    sync::Mutex,
    time::{sleep, Duration},
};

use crate::{
    database::Db,
    nostr::Nostr,
    types::{unix_time, PendingInvoice},
};

#[derive(Debug, Clone)]
pub struct Cashu {
    mints: Arc<Mutex<HashMap<String, Option<CashuWallet>>>>,
    db: Db,
    nostr: Nostr,
}

impl Cashu {
    pub fn new(db: Db, nostr: Nostr) -> Self {
        Self {
            mints: Arc::new(Mutex::new(HashMap::new())),
            db,
            nostr,
        }
    }

    /// Get wallet for uri
    async fn wallet_for_url(&self, mint_url: &str) -> Result<CashuWallet> {
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

                            if let Some(user) = user {
                                cashu
                                    .nostr
                                    .send_token(
                                        &user.pubkey,
                                        token,
                                        &user.relays.into_iter().collect(),
                                    )
                                    .await?;
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
        mint_url: &str,
    ) -> Result<RequestMintResponse> {
        debug!("Getting walletff");
        let wallet = self.wallet_for_url(mint_url).await?;
        debug!("Got wallet");
        let invoice = wallet.request_mint(amount).await?;

        Ok(invoice)
    }

    pub async fn mint(&self, pending_invoice: &PendingInvoice) -> Result<Token> {
        let wallet = self.wallet_for_url(&pending_invoice.mint).await?;

        let mint_response = wallet
            .mint_token(pending_invoice.amount, &pending_invoice.hash)
            .await?;

        Ok(mint_response)
    }

    pub async fn add_pending_invoice(&self, pending_invoice: &PendingInvoice) -> Result<()> {
        self.db
            .add_pending_invoice(&pending_invoice.hash, pending_invoice)
            .await?;

        Ok(())
    }
}
