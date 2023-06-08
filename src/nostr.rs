use std::collections::HashSet;
use std::{str::FromStr, sync::Arc};

use anyhow::Result;
use cashu_crab::types::Token;
use log::{debug, warn};
use nostr_sdk::prelude::*;
use tokio::sync::Mutex;
use tungstenite::Message as WsMessage;

use crate::database::Db;
use crate::types::{User, UserSignUp};

#[derive(Clone, Debug)]
pub struct Nostr {
    db: Db,
    keys: Keys,
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

        debug!("{:?}", keys.public_key().to_string());

        Ok(keys)
    }

    /// Init Nostr Client
    pub async fn new(
        db: Db,
        private_key: &Option<String>,
        relays: HashSet<String>,
    ) -> Result<Self> {
        let keys = Self::handle_keys(private_key)?;

        let client = Client::new(&keys);
        let nostr_relays = relays.iter().map(|url| (url, None)).collect();
        client.add_relays(nostr_relays).await?;
        client.connect().await;

        let subscription = Filter::new()
            .pubkey(keys.public_key())
            .kind(Kind::EncryptedDirectMessage)
            .since(Timestamp::now());

        client.subscribe(vec![subscription]).await;

        Ok(Self {
            db,
            keys,
            client: Arc::new(Mutex::new(Some(client))),
            relays,
        })
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
            client
                .handle_notifications(|notification| async {
                    if let RelayPoolNotification::Event(_url, event) = notification {
                        if event.kind == Kind::EncryptedDirectMessage {
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
                                                    let updated_user = User {
                                                        mint: user_info.mint,
                                                        pubkey: user.pubkey,
                                                        relays: user_info.relays,
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
                                                        )
                                                        .await?;
                                                } else {
                                                    client
                                                        .send_direct_msg(
                                                            event.pubkey,
                                                            "Username already taken",
                                                        )
                                                        .await?;
                                                }
                                            }
                                            None => {
                                                let new_user = User {
                                                    mint: user_info.mint,
                                                    pubkey: event.pubkey.to_string(),
                                                    relays: user_info.relays,
                                                };
                                                self.db
                                                    .add_user(&user_info.username, &new_user)
                                                    .await?;

                                                client
                                                    .send_direct_msg(
                                                        event.pubkey,
                                                        "Sign up complete",
                                                    )
                                                    .await?;
                                            }
                                        }
                                    }
                                }
                                Err(e) => log::error!("Impossible to decrypt direct message: {e}"),
                            }
                        }
                    }
                    Ok(false) // Set to true to exit from the loop
                })
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
                .write_message(WsMessage::Text(msg))
                .expect("Impossible to send message");
        }

        Ok(())
    }
}
