use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use anyhow::bail;
use axum::routing::{delete, get, post};
use axum::Router;
use cashu::Cashu;
use cashu_sdk::Amount;
use clap::Parser;
use cln_rpc::model::{
    requests::{PayRequest, WaitanyinvoiceRequest},
    responses::WaitanyinvoiceResponse,
};
use cln_rpc::primitives::Amount as CLN_Amount;
use cln_rpc::ClnRpc;
use database::Db;
use dirs::data_dir;
use futures::{Stream, StreamExt};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{debug, info, warn};
use types::{unix_time, PendingInvoice, PendingUser, UserKind};
use url::Url;

use crate::cli::CLIArgs;
use crate::config::{Info, Network, Settings};
use crate::nostr::Nostr;
use crate::routes::{
    delete_user, get_list_users, get_user_invoice, get_user_lnurl_struct, post_add_user,
    post_block_user, post_reserve_user, post_sign_up,
};

mod cashu;
mod cli;
mod config;
mod database;
mod error;
mod nostr;
mod routes;
mod types;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let args = CLIArgs::parse();

    let config_file_settings = match args.config {
        Some(config_path) => config::Settings::new(&Some(config_path)),
        None => Settings::default(),
    };

    let url = match args.url {
        Some(url) => url,
        None => config_file_settings.info.url,
    };

    let mint = args.mint.unwrap_or(config_file_settings.info.mint);

    let invoice_description = args
        .invoice_description
        .or(config_file_settings.info.invoice_description);

    let nostr_nsec = match args.nsec {
        Some(nsec) => Some(nsec),
        None => config_file_settings.info.nostr_nsec,
    };

    let relays = if args.relays.is_empty() {
        config_file_settings.info.relays
    } else {
        args.relays.into_iter().collect()
    };

    let max_sendable: Amount = args.max_sendable.map(Amount::from_sat).unwrap_or(
        config_file_settings
            .info
            .max_sendable
            .unwrap_or(Amount::from_sat(1000000)),
    );

    let min_sendable: Amount = args.max_sendable.map(Amount::from_sat).unwrap_or(
        config_file_settings
            .info
            .min_sendable
            .unwrap_or(Amount::from_sat(1)),
    );

    let db_path = args.db_path.or(config_file_settings.info.db_path);

    let proxy = args.proxy.unwrap_or(config_file_settings.info.proxy);

    let fee = args
        .fee
        .unwrap_or(config_file_settings.info.fee.unwrap_or(0.0));

    let cln_path = args.cln_path.or(config_file_settings.info.cln_path);

    let zapper = Some(
        args.zapper
            .unwrap_or(config_file_settings.info.zapper.unwrap_or_default()),
    );

    let pay_index_path = args
        .pay_index_path
        .or(config_file_settings.info.pay_index_path);

    let address = args.address.unwrap_or(config_file_settings.network.address);

    let port = args.port.unwrap_or(config_file_settings.network.port);

    let two_char_cost: Amount = args.two_char_price.map(Amount::from_sat).unwrap_or(
        config_file_settings
            .info
            .two_char_cost
            .unwrap_or(Amount::from_sat(0)),
    );

    let three_char_cost: Amount = args
        .three_char_price
        .map(Amount::from_sat)
        .unwrap_or(
            config_file_settings
                .info
                .three_char_cost
                .unwrap_or(Amount::from_sat(0)),
        );

    let four_char_cost: Amount = args.four_char_price.map(Amount::from_sat).unwrap_or(
        config_file_settings
            .info
            .four_char_cost
            .unwrap_or(Amount::from_sat(0)),
    );

    let other_char_cost: Amount = args
        .other_char_price
        .map(Amount::from_sat)
        .unwrap_or(
            config_file_settings
                .info
                .other_char_cost
                .unwrap_or(Amount::from_sat(0)),
        );

    let settings = Settings {
        info: Info {
            url,
            nostr_nsec,
            relays,
            mint,
            invoice_description,
            proxy,
            fee: Some(fee),
            cln_path,
            min_sendable: Some(min_sendable),
            max_sendable: Some(max_sendable),
            zapper,
            db_path,
            pay_index_path,
            two_char_cost: Some(two_char_cost),
            three_char_cost: Some(three_char_cost),
            four_char_cost: Some(three_char_cost),
            other_char_cost: Some(other_char_cost),
        },
        network: Network { port, address },
    };

    let api_base_address = Url::from_str(&settings.info.url)?;
    let description = match settings.info.invoice_description.clone() {
        Some(des) => des,
        None => "Hello World".to_string(),
    };
    let nostr_nsec = settings.info.nostr_nsec.clone();
    let relays = settings.info.relays.clone();

    debug!("Relays: {:?}", relays);

    if relays.is_empty() {
        bail!("Must define at least one relay");
    }

    let db_path = match settings.info.db_path.clone() {
        Some(path) => PathBuf::from_str(&path)?,
        None => {
            let data_dir = dirs::data_dir().ok_or(anyhow!("Could not get data dir".to_string()))?;
            data_dir.join("cashu-lnurl")
        }
    };

    let db = Db::new(db_path).await?;

    let nostr = Nostr::new(
        db.clone(),
        api_base_address.to_string(),
        &nostr_nsec,
        relays,
    )
    .await?;

    let cashu = Cashu::new(db.clone(), nostr.clone(), settings.clone());

    let mut nostr_clone = nostr.clone();
    let nostr_task = tokio::spawn(async move { nostr_clone.run().await });

    let cashu_clone = cashu.clone();
    let cashu_task = tokio::spawn(async move { cashu_clone.run().await });

    let cln_client = if let Some(cln_path) = settings.info.cln_path.clone() {
        Arc::new(Mutex::new(Some(ClnRpc::new(cln_path).await?)))
    } else {
        Arc::new(Mutex::new(None))
    };

    let db_clone = db.clone();
    let cashu_clone = cashu.clone();
    let cln_client_clone = cln_client.clone();

    let pending_users = Arc::new(Mutex::new(
        db_clone
            .get_pending_users()
            .await?
            .into_iter()
            .map(|u| (u.pr.payment_hash().to_string(), u))
            .collect(),
    ));

    let state = LnurlState {
        api_base_address,
        min_sendable,
        max_sendable,
        description,
        nostr_pubkey: Some(nostr.get_pubkey()),
        proxy: settings.info.proxy,
        cashu,
        db,
        cln_client,
        _nostr: nostr,
        pending_users: pending_users.clone(),
        two_char_cost,
        three_char_cost,
        four_char_cost,
        other_char_cost,
    };

    let lnurl_service = Router::new()
        .route("/.well-known/lnurlp/:username", get(get_user_lnurl_struct))
        .route("/lnurlp/:username/invoice", get(get_user_invoice))
        .route("/signup", post(post_sign_up))
        .route("/add_user", post(post_add_user))
        .route("/remove_user", delete(delete_user))
        .route("/list_users", get(get_list_users))
        .route("/reserve", post(post_reserve_user))
        .route("/block", post(post_block_user))
        .with_state(state);

    let address = settings.network.address;
    let ip = Ipv4Addr::from_str(&address)?;

    let port = settings.network.port;

    let listen_addr = SocketAddr::new(std::net::IpAddr::V4(ip), port);

    let axum_task = axum::Server::bind(&listen_addr).serve(lnurl_service.into_make_service());

    // Task that waits for invoice to be paid
    // When an invoice paid check db if invoice exists request mint and pay and mint
    // DM tokens to user

    if settings.info.proxy
        | ((two_char_cost + three_char_cost + four_char_cost + other_char_cost).gt(&Amount::ZERO))
    {
        let rpc_socket = settings
            .info
            .cln_path
            .clone()
            .expect("CLN RPC socket path required");
        let pending_users_clone = pending_users.clone();

        let wait_invoice_task = tokio::spawn(async move {
            let pay_index_path = match settings.info.pay_index_path {
                Some(path) => path,
                None => index_file_path().expect("Could not get path to pay index file"),
            };

            let last_pay_index = match read_last_pay_index(&pay_index_path) {
                Ok(idx) => idx,
                Err(e) => {
                    warn!("Could not read last pay index: {e}");
                    if let Err(e) = write_last_pay_index(&pay_index_path, 0) {
                        warn!("Write error: {e}");
                    }
                    0
                }
            };
            info!("Starting at pay index: {last_pay_index}");

            let mut invoices = invoice_stream(&rpc_socket, pay_index_path, Some(last_pay_index))
                .await
                .unwrap();
            let db = db_clone;
            let cashu = cashu_clone;
            let cln_client = cln_client_clone;

            while let Some((hash, _invoice)) = invoices.next().await {
                // Check if invoice is for a pending user

                let mut pending = pending_users.lock().await;
                if let Some(pending_user) = pending.get(&hash) {
                    debug!("Invoice for pending user paid: {:?}", pending_user);
                    if let Err(err) = db
                        .add_user(
                            &pending_user.user.username,
                            &UserKind::User(pending_user.user.clone()),
                        )
                        .await
                    {
                        warn!(
                            "Could not move pending user to user {}: {:?}",
                            pending_user.user.username, err
                        );
                    }

                    pending.remove(&hash);
                }
                // Check if invoice is in db and proxied
                // If it is request mint from selected mint
                else if let Ok(Some(invoice)) = db.get_pending_invoice(&hash).await {
                    drop(pending);
                    // Fee to account for routing fee

                    let fee = fee_for_invoice(invoice.amount, settings.info.fee.unwrap_or(0.0));

                    if let Err(err) = db.add_fee_received(&invoice.hash, fee.to_msat()).await {
                        warn!("Could not add received fee to DB: {:?}", err);
                        info!("Fee received: {:?}", fee.to_msat());
                    }

                    // In the case of small invoices that will likely not incur a routing fee
                    // No fee is taken, This should be configurable.
                    // However it must be ensured that it is always
                    // > 1 sat as that is the min for cashu tokens
                    // In this small case a max fee of 10 sats is set.
                    // As I would rather the service eat the fees
                    // TO avoid the poor user experience of failed payments
                    let max_fee = if fee.eq(&Amount::ZERO) {
                        Amount::from_sat(10)
                    } else {
                        fee
                    };

                    let amount = invoice.amount - fee;

                    let request_mint_response =
                        match cashu.request_mint(amount, &invoice.mint).await {
                            Ok(res) => res,
                            Err(err) => {
                                warn!("{:?}", err);
                                continue;
                            }
                        };

                    let pending_invoice = PendingInvoice {
                        mint: invoice.mint,
                        username: invoice.username,
                        description: invoice.description,
                        amount,
                        hash: request_mint_response.hash,
                        bolt11: request_mint_response.pr.clone(),
                        last_checked: None,
                        proxied: true,
                        time: unix_time(),
                    };

                    // Add mint pending ivoice to DB
                    if let Err(err) = cashu.add_pending_invoice(&pending_invoice).await {
                        warn!("Could not add pending invoice: {:?}", err)
                    }

                    // Remove paid invoice from pending
                    if let Err(err) = db.remove_pending_invoice(&invoice.hash).await {
                        warn!("Could not remove pending invoice {:?}", err);
                    }

                    // Pay mint invoice
                    let mut cln_client = cln_client.lock().await;

                    let cln_response = cln_client
                        .as_mut()
                        .unwrap()
                        .call(cln_rpc::Request::Pay(PayRequest {
                            bolt11: request_mint_response.pr.to_string(),
                            amount_msat: None,
                            label: None,
                            riskfactor: None,
                            maxfeepercent: None,
                            retry_for: None,
                            maxdelay: None,
                            exemptfee: None,
                            localinvreqid: None,
                            exclude: None,
                            maxfee: Some(CLN_Amount::from_sat(max_fee.to_sat())),
                            description: None,
                        }))
                        .await;

                    match cln_response {
                        Ok(cln_rpc::Response::Pay(pay_response)) => {
                            if let Ok(pay_response) =
                                serde_json::to_string(&pay_response.payment_preimage)
                            {
                                // let invoice = Amount::from_msat(pay_response.amount_sent_msat.msat());
                                debug!("Invoice paid: {:?}", pay_response);
                            }
                            if let Err(err) = db
                                .add_fee_paid(
                                    &pay_response.payment_hash.to_string(),
                                    (pay_response.amount_sent_msat - pay_response.amount_msat)
                                        .msat(),
                                )
                                .await
                            {
                                warn!("Could not add paid fee to DB: {:?}", err);

                                info!(
                                    "Fee Paid: {:?}",
                                    pay_response.amount_sent_msat - pay_response.amount_msat
                                );
                            }
                        }
                        Ok(res) => warn!("Wrong CLN response: {:?}", res),
                        Err(err) => warn!("Error paying mint invoice: {:?}", err),
                    };
                }
            }
        });

        let remove_expired_pending_users_task = tokio::spawn(async move {
            loop {
                let mut pending_users = pending_users_clone.lock().await;

                let current_time = unix_time();

                let pending_users_count = pending_users.len();

                pending_users.retain(|_k, v| v.expire.gt(&current_time));
                debug!(
                    "Removed {} expired pending users.",
                    pending_users_count - pending_users.len()
                );
                drop(pending_users);

                sleep(Duration::from_secs(15)).await;
            }
        });

        tokio::select! {
            _ = nostr_task => {
                warn!("Nostr task ended");
            }
            _ = cashu_task => {
                warn!("Cashu task ended");
            }
            _ = axum_task => {
                warn!("Axum task ended");
            }
            _ = wait_invoice_task => {
                warn!("Wait invoice task ended");
            }
            _ = remove_expired_pending_users_task => {
                warn!("Remove expired users task ended")
            }
        }
    } else {
        tokio::select! {
            _ = nostr_task => {
                warn!("Nostr task ended");
            }
            _ = cashu_task => {
                warn!("Cashu task ended");
            }
            _ = axum_task => {
                warn!("Axum task ended");
            }
        }
    }

    Ok(())
}

async fn invoice_stream(
    socket_addr: &str,
    pay_index_path: PathBuf,
    last_pay_index: Option<u64>,
) -> anyhow::Result<impl Stream<Item = (String, WaitanyinvoiceResponse)>> {
    let cln_client = cln_rpc::ClnRpc::new(&socket_addr).await?;

    Ok(futures::stream::unfold(
        (cln_client, pay_index_path, last_pay_index),
        |(mut cln_client, pay_index_path, mut last_pay_idx)| async move {
            // We loop here since some invoices aren't zaps, in which case we wait for the next one and don't yield
            loop {
                // info!("Waiting for index: {last_pay_idx:?}");
                let invoice_res = cln_client
                    .call(cln_rpc::Request::WaitAnyInvoice(WaitanyinvoiceRequest {
                        timeout: None,
                        lastpay_index: last_pay_idx,
                    }))
                    .await;

                let invoice: WaitanyinvoiceResponse = match invoice_res {
                    Ok(invoice) => invoice,
                    Err(e) => {
                        warn!("Error fetching invoice: {e}");
                        // Let's not spam CLN with requests on failure
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        // Retry same request
                        continue;
                    }
                }
                .try_into()
                .expect("Wrong response from CLN");

                last_pay_idx = invoice.pay_index;
                if let Some(idx) = last_pay_idx {
                    if let Err(e) = write_last_pay_index(&pay_index_path, idx) {
                        warn!("Could not write index tip: {e}");
                    }
                };
                let pay_idx = last_pay_idx;

                break Some((
                    (invoice.payment_hash.to_string(), invoice),
                    (cln_client, pay_index_path, pay_idx),
                ));
            }
        },
    )
    .boxed())
}

/// Calculate fee for invoice
// REVIEW: This is a fairly naive way to handle fees
// Simply takes 1%
fn fee_for_invoice(amount: Amount, fee_percent: f32) -> Amount {
    Amount::from_msat((amount.to_msat() as f32 * fee_percent).ceil() as u64)
}

/// Default file path for last pay index tip
fn index_file_path() -> anyhow::Result<PathBuf> {
    let mut file_path = match data_dir() {
        Some(path) => path,
        None => return Err(anyhow!("no data dir")),
    };

    file_path.push("cln-zapper");
    file_path.push("last_pay_index");

    Ok(file_path)
}

/// Read last pay index tip from file
fn read_last_pay_index(file_path: &PathBuf) -> anyhow::Result<u64> {
    let mut file = File::open(file_path)?;
    let mut buffer = [0; 8];

    file.read_exact(&mut buffer)?;
    Ok(u64::from_ne_bytes(buffer))
}

/// Write last pay index tip to file
fn write_last_pay_index(file_path: &PathBuf, last_pay_index: u64) -> anyhow::Result<()> {
    // Create the directory if it doesn't exist
    if let Some(parent_dir) = file_path.parent() {
        fs::create_dir_all(parent_dir)?;
    }

    let mut file = File::create(file_path)?;
    file.write_all(&last_pay_index.to_ne_bytes())?;
    Ok(())
}

#[derive(Clone)]
pub struct LnurlState {
    api_base_address: Url,
    min_sendable: Amount,
    max_sendable: Amount,
    description: String,
    nostr_pubkey: Option<String>,
    // If proxied cashu-lnurl created the invoice
    proxy: bool,
    cashu: Cashu,
    cln_client: Arc<Mutex<Option<ClnRpc>>>,
    db: Db,
    _nostr: Nostr,
    pending_users: Arc<Mutex<HashMap<String, PendingUser>>>,
    two_char_cost: Amount,
    three_char_cost: Amount,
    four_char_cost: Amount,
    other_char_cost: Amount,
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_fee_calculation() {
        let amount = Amount::from_sat(1);

        assert_eq!(fee_for_invoice(amount, 0.0), Amount::ZERO);

        let amount = Amount::from_sat(100);

        assert_eq!(fee_for_invoice(amount, 0.01), Amount::from_sat(1));
    }
}
