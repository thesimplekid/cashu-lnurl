use std::collections::HashSet;
use std::time::Duration;
use std::{str::FromStr, sync::Arc};

use anyhow::{bail, Result};
use cashu_sdk::nuts::nut00::wallet::Token;
use cashu_sdk::Bolt11Invoice;
use nostr_sdk::prelude::*;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};
use tungstenite::Message as WsMessage;

use crate::database::Db;
use crate::types::{User, UserSignUp};

const SIGNUP_KIND: u64 = 20420;

#[derive(Clone, Debug)]
pub struct Nostr {
    db: Db,
    keys: Keys,
    domain: String,
    client: Arc<Mutex<Option<Client>>>,
    relays: HashSet<String>,
}

impl Nostr {
    /// Convert string key to nostr keys
    fn handle_keys(private_key: &Option<String>) -> Result<Keys> {
        // Parse and validate private key
        let keys = match private_key {
            Some(pk) => {
                // create a new identity using the provided private key
                Keys::from_sk_str(pk.as_str())?
            }
            None => Keys::generate(),
        };

        debug!("Public key {:?}", keys.public_key().to_string());

        Ok(keys)
    }

    pub fn get_pubkey(&self) -> String {
        self.keys.public_key().to_string()
    }

    /// Init Nostr Client
    pub async fn new(
        db: Db,
        domain: String,
        private_key: &Option<String>,
        relays: HashSet<String>,
    ) -> Result<Self> {
        let keys = Self::handle_keys(private_key)?;

        let client = Client::new(&keys);
        let nostr_relays = relays.iter().map(|url| (url.to_string(), None)).collect();
        client.add_relays(nostr_relays).await?;

        Ok(Self {
            db,
            domain,
            keys,
            client: Arc::new(Mutex::new(Some(client))),
            relays,
        })
    }

    async fn get_user_relays(client: &Client, pubkey: &str) -> Result<HashSet<String>> {
        let filter = Filter::new().author(pubkey).kind(Kind::ContactList);

        let events = client
            .get_events_of(vec![filter], Some(Duration::from_secs(10)))
            .await?;
        let most_recent = events.iter().max_by_key(|event| event.created_at);

        let mut relays = HashSet::new();

        if let Some(event) = most_recent {
            let content: Value = serde_json::from_str(&event.content)?;
            let content = content.as_object();
            if let Some(relay) = content {
                relays = relay.keys().map(|r| r.to_string()).collect();
            }
        }

        Ok(relays)
    }

    /// Perform Nostr tasks
    pub async fn run(&mut self) -> Result<()> {
        loop {
            let res = self.run_internal().await;
            if let Err(e) = res {
                warn!("Run error: {:?}", e);
            }
        }
    }

    /// Internal select loop for preforming nostr operations
    async fn run_internal(&mut self) -> Result<()> {
        let mut client_guard = self.client.lock().await;
        if let Some(client) = client_guard.as_mut() {
            client.connect().await;
            let keys = client.keys();

            let subscription = Filter::new()
                .pubkey(keys.public_key())
                .kind(Kind::Custom(SIGNUP_KIND));

            client.subscribe(vec![subscription]).await;

            client
                .handle_notifications(|notification| async {
                    if let RelayPoolNotification::Event(_url, event) = notification {
                        debug!("Got event: {:?}", event.as_json());
                        if event.kind == Kind::Custom(SIGNUP_KIND) {
                            match decrypt(
                                &client.keys().secret_key()?,
                                &event.pubkey,
                                &event.content,
                            ) {
                                Ok(msg) => {
                                    debug!("MSG Content: {}", msg);
                                    if let Ok(user_info) = serde_json::from_str::<UserSignUp>(&msg)
                                    {
                                        // Check if user exists
                                        match self.db.get_user(&user_info.username).await? {
                                            Some(user) => {
                                                if user.pubkey.eq(&event.pubkey.to_string()) {
                                                    let relays =
                                                        Self::get_user_relays(client, &user.pubkey)
                                                            .await?;

                                                    debug!("User relays: {:?}", relays);

                                                    let updated_user = User {
                                                        mint: user_info.mint,
                                                        pubkey: user.pubkey,
                                                        proxy: user.proxy,
                                                        relays,
                                                    };

                                                    self.db
                                                        .add_user(
                                                            &user_info.username,
                                                            &updated_user,
                                                        )
                                                        .await?;

                                                    client
                                                        .send_direct_msg(
                                                            event.pubkey,
                                                            "Mints and Relays updated",
                                                            None,
                                                        )
                                                        .await?;
                                                } else {
                                                    client
                                                        .send_direct_msg(
                                                            event.pubkey,
                                                            "Username already taken",
                                                            None,
                                                        )
                                                        .await?;
                                                }
                                            }
                                            None => {
                                                let relays = Self::get_user_relays(
                                                    client,
                                                    &event.pubkey.to_string(),
                                                )
                                                .await?;

                                                debug!("User relays: {:?}", relays);
                                                let new_user = User {
                                                    mint: user_info.mint,
                                                    pubkey: event.pubkey.to_string(),
                                                    // TODO: Need to change nostr to allow this be configured
                                                    proxy: true,
                                                    relays,
                                                };

                                                self.db
                                                    .add_user(&user_info.username, &new_user)
                                                    .await?;

                                                client
                                                    .send_direct_msg(
                                                        event.pubkey,
                                                        self.sign_up_message(
                                                            &user_info.username,
                                                            &new_user,
                                                        ),
                                                        None,
                                                    )
                                                    .await?;
                                            }
                                        }
                                    }
                                }
                                Err(e) => error!("Impossible to decrypt direct message: {e}"),
                            }
                        }
                    }
                    Ok(false) // Set to true to exit from the loop
                })
                .await?;
        }
        Ok(())
    }

    fn sign_up_message(&self, username: &str, user: &User) -> String {
        format!(
            "Welcome! \n You're ln address is {}@{}.\n You will get cashu tokens from mint {}",
            username, self.domain, user.mint
        )
    }

    pub async fn send_sign_up_message(&self, username: &str, user: &User) -> anyhow::Result<()> {
        let mut client_guard = self.client.lock().await;
        if let Some(client) = client_guard.as_mut() {
            client
                .send_direct_msg(
                    XOnlyPublicKey::from_str(&user.pubkey)?,
                    self.sign_up_message(username, user),
                    None,
                )
                .await?;
        }
        Ok(())
    }

    pub async fn send_token(
        &self,
        receiver: &str,
        token: Token,
        relays: &HashSet<String>,
    ) -> Result<()> {
        let receiver = XOnlyPublicKey::from_str(receiver)?;

        let event = EventBuilder::new_encrypted_direct_msg(
            &self.keys,
            receiver,
            token.convert_to_string()?,
            None,
        )?
        .to_event(&self.keys)?;

        self.broadcast_event(relays, event).await?;
        Ok(())
    }

    async fn broadcast_event(&self, relays: &HashSet<String>, event: Event) -> Result<()> {
        let relays: HashSet<&String> = relays.union(&self.relays).collect();
        debug!("{:?}", relays);
        for relay in relays {
            let mut socket = match tungstenite::connect(relay) {
                Ok((s, _)) => s,
                // TODO: the mutiny relay returns an http 200 its getting logged as an error
                Err(err) => {
                    warn!("Error connecting to {relay}: {err}");
                    continue;
                }
            };

            // Send msg
            let msg = ClientMessage::new_event(event.clone()).as_json();
            socket
                .send(WsMessage::Text(msg))
                .expect("Impossible to send message");
        }

        Ok(())
    }

    pub async fn broadcast_zap(
        &self,
        bolt11: Bolt11Invoice,
        description: &str,
        relays: &HashSet<String>,
    ) -> Result<()> {
        let zap_request = Event::from_json(description)?;

        let mut request_relays: HashSet<String> = zap_request
            .tags
            .iter()
            .filter_map(|tag| match tag {
                Tag::Relays(values) => Some(
                    values
                        .iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<String>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect();

        debug!("req relays {:?}", request_relays);

        request_relays.extend(relays.clone());

        if zap_request.kind.ne(&Kind::ZapRequest) {
            bail!("Description is not a zap request");
        }

        let zap_event = EventBuilder::new_zap_receipt(bolt11.to_string(), None, zap_request)
            .to_event(&self.keys)?;
        debug!("{:?}", zap_event.as_json());
        self.broadcast_event(&request_relays, zap_event).await?;

        Ok(())
    }
}
