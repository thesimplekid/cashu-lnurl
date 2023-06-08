use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use anyhow::bail;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use cashu::Cashu;
use cashu_crab::Amount;
use database::Db;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use types::{as_msat, PendingInvoice};
use url::Url;

use crate::nostr::Nostr;
use crate::utils::amount_from_msat;

mod cashu;
mod config;
mod database;
mod nostr;
mod types;
mod utils;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let settings = config::Settings::new(&None);

    let api_base_address = Url::from_str(&settings.info.url)?;
    let description = match settings.info.invoice_description {
        Some(des) => des,
        None => "Hello World".to_string(),
    };
    let nostr_pubkey = settings.info.nostr_nsec;
    let relays = settings.info.relays;

    if relays.is_empty() {
        bail!("Must define at least one relay");
    }

    let data_dir = dirs::data_dir().unwrap();
    let data = data_dir.join("cashu-lnurl");

    let db = Db::new(data).await?;
    let nostr = Nostr::new(db.clone(), &nostr_pubkey, relays).await?;
    let cashu = Cashu::new(db.clone(), nostr.clone());

    let mut nostr_clone = nostr.clone();
    tokio::spawn(async move { nostr_clone.run().await });

    let cashu_clone = cashu.clone();
    tokio::spawn(async move { cashu_clone.run().await });

    let state = LnurlState {
        api_base_address,
        min_sendable: Amount::from_sat(0),
        max_sendable: Amount::from_sat(1000000),
        description,
        nostr_pubkey,
        cashu,
        db,
    };

    let lnurl_service = Router::new()
        .route("/.well-known/lnurlp/:username", get(get_user_lnurl_struct))
        .route("/lnurlp/:username/invoice", get(get_user_invoice))
        .with_state(state);

    let listen_addr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);

    axum::Server::bind(&listen_addr)
        .serve(lnurl_service.into_make_service())
        .await?;

    Ok(())
}

async fn get_user_lnurl_struct(
    State(state): State<LnurlState>,
    Path(username): Path<String>,
) -> Result<Json<LnurlResponse>, StatusCode> {
    let _user = match state.db.get_user(&username).await {
        Ok(Some(user)) => user,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(err) => {
            warn!("{:?}", err);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let mut callback = state
        .api_base_address
        .join("lnurlp")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    callback
        .path_segments_mut()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .push(&username);
    callback
        .path_segments_mut()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .push("invoice");

    Ok(Json(LnurlResponse {
        min_sendable: state.min_sendable,
        max_sendable: state.max_sendable,
        metadata: serde_json::to_string(&vec![vec!["text/plain".to_string(), state.description]])
            .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?,
        callback,
        tag: LnurlTag::PayRequest,
        allows_nostr: state.nostr_pubkey.is_some(),
        nostr_pubkey: state.nostr_pubkey,
    }))
}

async fn get_user_invoice(
    Query(params): Query<GetInvoiceParams>,
    Path(username): Path<String>,
    State(state): State<LnurlState>,
) -> Result<Json<GetInvoiceResponse>, StatusCode> {
    let db = state.db;

    let user = match db.get_user(&username).await {
        Ok(Some(user)) => user,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(err) => {
            warn!("{:?}", err);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let mint = &user.mint;
    let amount = amount_from_msat(params.amount);
    debug!("{}", amount.to_sat());
    let request_mint_response = state
        .cashu
        .request_mint(amount, mint)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let pending_invoice = PendingInvoice::new(
        mint,
        &username,
        params.nostr,
        amount_from_msat(params.amount),
        &request_mint_response.hash,
        request_mint_response.pr.clone(),
        None,
    );

    state
        .cashu
        .add_pending_invoice(&pending_invoice)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(GetInvoiceResponse {
        pr: request_mint_response.pr.to_string(),
        success_action: None,
        routes: vec![],
    }))
}

#[derive(Debug, Serialize, Deserialize)]
struct GetInvoiceParams {
    amount: u64,
    nostr: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetInvoiceResponse {
    pr: String,
    // TODO: find out proper type
    success_action: Option<String>,
    // TODO: find out proper type
    routes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum LnurlTag {
    PayRequest,
}

#[derive(Debug, Clone)]
struct LnurlState {
    api_base_address: Url,
    min_sendable: Amount,
    max_sendable: Amount,
    description: String,
    nostr_pubkey: Option<String>,
    cashu: Cashu,
    db: Db,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct LnurlResponse {
    #[serde(with = "as_msat")]
    min_sendable: Amount,
    #[serde(with = "as_msat")]
    max_sendable: Amount,
    metadata: String,
    callback: Url,
    tag: LnurlTag,
    allows_nostr: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    nostr_pubkey: Option<String>,
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_lnurl_response_serialization() {
        let lnurl_response = LnurlResponse {
            min_sendable: Amount::from_sat(0),
            max_sendable: Amount::from_sat(1000),
            metadata: serde_json::to_string(&vec![vec![
                "text/plain".to_string(),
                "Hello world".to_string(),
            ]])
            .unwrap(),
            callback: Url::from_str("http://example.com").unwrap(),
            tag: LnurlTag::PayRequest,
            allows_nostr: true,
            nostr_pubkey: Some(
                "9630f464cca6a5147aa8a35f0bcdd3ce485324e732fd39e09233b1d848238f31".to_string(),
            ),
        };

        assert_eq!("{\"minSendable\":0,\"maxSendable\":1000000,\"metadata\":\"[[\\\"text/plain\\\",\\\"Hello world\\\"]]\",\"callback\":\"http://example.com/\",\"tag\":\"payRequest\",\"allowsNostr\":true,\"nostrPubkey\":\"9630f464cca6a5147aa8a35f0bcdd3ce485324e732fd39e09233b1d848238f31\"}", serde_json::to_string(&lnurl_response).unwrap());
    }
}
