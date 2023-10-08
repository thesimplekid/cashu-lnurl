use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use cashu_sdk::{Amount, Bolt11Invoice};
use cln_rpc::model::requests::InvoiceRequest;
use cln_rpc::primitives::{Amount as CLN_Amount, AmountOrAny};
use cln_rpc::ClnRpc;
use nostr_sdk::secp256k1::XOnlyPublicKey;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, error, warn};
use url::Url;
use uuid::Uuid;

use crate::types::{as_msat, unix_time, PendingInvoice, PendingUser, User, UserKind};
use crate::LnurlState;

/// List all users
pub(crate) async fn get_list_users(
    State(state): State<LnurlState>,
) -> Result<Json<Vec<User>>, StatusCode> {
    let users = state.db.get_all_users().await.map_err(|err| {
        warn!("Could not get users: {:?}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(users))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveParams {
    username: String,
    cost: Amount,
}

pub(crate) async fn post_reserve_user(
    State(state): State<LnurlState>,
    Query(params): Query<ReserveParams>,
) -> Result<StatusCode, StatusCode> {
    state
        .db
        .add_user(&params.username, &UserKind::Reserved(params.cost))
        .await
        .map_err(|err| {
            warn!("Could not reserve user: {:?}", err);
            StatusCode::OK
        })?;

    Ok(StatusCode::OK)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockParams {
    username: String,
}

pub(crate) async fn post_block_user(
    State(state): State<LnurlState>,
    Query(params): Query<BlockParams>,
) -> Result<StatusCode, StatusCode> {
    state
        .db
        .add_user(&params.username, &UserKind::Blocked)
        .await
        .map_err(|err| {
            warn!("Could not reserve user: {:?}", err);
            StatusCode::OK
        })?;

    Ok(StatusCode::OK)
}

/// Add user overwriting is already in db
pub(crate) async fn post_add_user(
    State(state): State<LnurlState>,
    Json(user): Json<User>,
) -> Result<StatusCode, StatusCode> {
    state
        .db
        .add_user(&user.username, &UserKind::User(user.clone()))
        .await
        .map_err(|err| {
            warn!("Could not add user: {:?}", err);
            StatusCode::OK
        })?;

    Ok(StatusCode::OK)
}

/// Delete User
pub(crate) async fn delete_user(
    State(state): State<LnurlState>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state.db.delete_user(&username).await.map_err(|err| {
        warn!("Could not delete user: {:?}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LnurlTag {
    PayRequest,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LnurlResponse {
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

pub(crate) async fn get_user_lnurl_struct(
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
            .map_err(|err| {
            warn!("{err}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?,
        callback,
        tag: LnurlTag::PayRequest,
        allows_nostr: state.nostr_pubkey.is_some(),
        nostr_pubkey: state.nostr_pubkey,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInvoiceParams {
    amount: u64,
    nostr: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetInvoiceResponse {
    pr: String,
    // TODO: find out proper type
    success_action: Option<String>,
    // TODO: find out proper type
    routes: Vec<String>,
}

pub(crate) async fn get_user_invoice(
    Query(params): Query<GetInvoiceParams>,
    Path(username): Path<String>,
    State(state): State<LnurlState>,
) -> Result<Json<GetInvoiceResponse>, StatusCode> {
    let db = state.db;

    let user = match db.get_user(&username).await {
        Ok(Some(UserKind::User(user))) => user,
        Ok(_) => {
            debug!("User {} is pending, invoice has not been paid.", username);
            return Err(StatusCode::NOT_FOUND);
        }
        Err(err) => {
            warn!("{:?}", err);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let mint = &user.mint;
    let amount = Amount::from_msat(params.amount);

    let pending_invoice = if state.proxy && user.proxy {
        let client = state.cln_client.clone();

        let cln_response = client
            .lock()
            .await
            .as_mut()
            .unwrap()
            .call(cln_rpc::Request::Invoice(InvoiceRequest {
                amount_msat: AmountOrAny::Amount(CLN_Amount::from_sat(amount.to_sat())),
                description: params.nostr.clone().unwrap_or_default(),
                label: Uuid::new_v4().to_string(),
                expiry: None,
                fallbacks: None,
                preimage: None,
                cltv: None,
                deschashonly: Some(true),
            }))
            .await;

        match cln_response {
            Ok(cln_rpc::Response::Invoice(invoice_response)) => {
                let invoice = Bolt11Invoice::from_str(&invoice_response.bolt11).unwrap();
                let pending_invoice = PendingInvoice {
                    mint: mint.clone(),
                    username,
                    description: params.clone().nostr,
                    amount: Amount::from_msat(params.amount),
                    time: unix_time(),
                    hash: invoice_response.payment_hash.to_string(),
                    bolt11: invoice,
                    last_checked: Some(unix_time()),
                    proxied: true,
                };
                state
                    .cashu
                    .add_pending_invoice(&pending_invoice)
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                Ok(pending_invoice)
            }
            Ok(res) => {
                warn!("Returned Wrong Cln response: {:?}", res);
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
            Err(err) => {
                error!("CLN RPC error: {:?}", err);
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    } else {
        let request_mint_response =
            state
                .cashu
                .request_mint(amount, mint)
                .await
                .map_err(|err| {
                    warn!("{:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        Ok(PendingInvoice {
            mint: mint.clone(),
            username,
            description: params.nostr,
            amount: Amount::from_msat(params.amount),
            hash: request_mint_response.hash,
            bolt11: request_mint_response.pr,
            last_checked: None,
            proxied: false,
            time: unix_time(),
        })
    };

    match pending_invoice {
        Ok(invoice) => Ok(Json(GetInvoiceResponse {
            pr: invoice.bolt11.to_string(),
            success_action: None,
            routes: vec![],
        })),
        Err(err) => Err(err),
    }
}

async fn get_invoice(
    client: Arc<Mutex<Option<ClnRpc>>>,
    amount: Amount,
    description: String,
) -> Result<Bolt11Invoice, StatusCode> {
    let cln_response = client
        .lock()
        .await
        .as_mut()
        .unwrap()
        .call(cln_rpc::Request::Invoice(InvoiceRequest {
            amount_msat: AmountOrAny::Amount(CLN_Amount::from_sat(amount.to_sat())),
            description,
            label: Uuid::new_v4().to_string(),
            expiry: None,
            fallbacks: None,
            preimage: None,
            cltv: None,
            deschashonly: Some(true),
        }))
        .await;

    match cln_response {
        Ok(cln_rpc::Response::Invoice(invoice_response)) => {
            Ok(Bolt11Invoice::from_str(&invoice_response.bolt11)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
        }
        Ok(res) => {
            warn!("Returned Wrong Cln response: {:?}", res);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(err) => {
            error!("CLN RPC error: {:?}", err);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignupParams {
    username: String,
    pubkey: XOnlyPublicKey,
    proxy: Option<bool>,
    mint: Url,
    relays: Option<HashSet<String>>,
}

pub(crate) async fn post_sign_up(
    State(state): State<LnurlState>,
    Json(params): Json<SignupParams>,
) -> Result<Json<String>, StatusCode> {
    match state.db.get_user(&params.username).await.map_err(|err| {
        error!("Could not get user: {:?}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })? {
        Some(UserKind::User(_user)) => Err(StatusCode::CONFLICT),
        Some(UserKind::Pending(_user)) => Err(StatusCode::CONFLICT),
        Some(UserKind::Blocked) => Err(StatusCode::NOT_ACCEPTABLE),
        Some(UserKind::Reserved(amount)) => {
            let client = state.cln_client.clone();

            let invoice =
                get_invoice(client, amount, format!("Payment for {}", params.username)).await?;

            let user = User {
                username: params.username.clone(),
                mint: params.mint,
                pubkey: params.pubkey.to_string(),
                relays: params.relays.unwrap_or_default(),
                proxy: params.proxy.unwrap_or_default(),
            };

            let pending_user = PendingUser {
                user,
                pr: invoice.clone(),
                last_checked: unix_time(),
                expire: unix_time() + 900,
            };

            let mut pending_users = state.pending_users.lock().await;
            pending_users.insert(invoice.payment_hash().to_string(), pending_user.clone());

            let pending_user = UserKind::Pending(pending_user);
            state
                .db
                .add_user(&params.username, &pending_user)
                .await
                .unwrap();

            Ok(Json(invoice.to_string()))
        }
        None => {
            let relays = params.relays.unwrap_or_default();
            let proxy = params.proxy.unwrap_or_default();

            let user = User {
                username: params.username.clone(),
                mint: params.mint,
                pubkey: params.pubkey.to_string(),
                relays,
                proxy,
            };

            let amount = if params.username.len().le(&2) {
                state.two_char_cost
            } else if params.username.len().le(&3) {
                state.three_char_cost
            } else if params.username.len().le(&4) {
                state.four_char_cost
            } else {
                state.other_char_cost
            };

            let user = if amount.gt(&Amount::ZERO) {
                let client = state.cln_client.clone();

                let pr = get_invoice(client, amount, format!("{}", params.username)).await?;
                let pending_user = PendingUser {
                    user: user.clone(),
                    pr: pr.clone(),
                    last_checked: unix_time(),
                    expire: unix_time() + 900,
                };

                let mut pending_users = state.pending_users.lock().await;
                pending_users.insert(pr.payment_hash().to_string(), pending_user.clone());

                UserKind::Pending(pending_user)
            } else {
                UserKind::User(user)
            };

            state.db.add_user(&params.username, &user).await.unwrap();

            /*
                        let nostr = state.nostr.clone();

                        let _ = thread::spawn(move || {
                            let _ = tokio::runtime::Runtime::new()
                                .unwrap()
                                .block_on(nostr.send_sign_up_message(&params.username, &user));
                        });
            */

            match user {
                UserKind::User(_) => Ok(Json("Ok".to_string())),
                UserKind::Pending(user) => Ok(Json(user.pr.to_string())),
                _ => {
                    warn!("Unexpected user type");
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
    }
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
