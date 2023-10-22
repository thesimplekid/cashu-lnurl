use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;
use tracing::warn;

use crate::types::{PendingInvoice, PendingUser, User, UserKind};

const USERS: TableDefinition<&str, &str> = TableDefinition::new("mint_info");

const PENDING: TableDefinition<&str, &str> = TableDefinition::new("pending");

const PAID_FEES: TableDefinition<&str, u64> = TableDefinition::new("paid_fees");

const RECEIVED_FEES: TableDefinition<&str, u64> = TableDefinition::new("received_fees");

#[derive(Debug, Clone)]
pub struct Db {
    db: Arc<Mutex<Database>>,
}

impl Db {
    /// Init Database
    pub async fn new(path: PathBuf) -> Result<Self> {
        let directory = path
            .parent()
            .ok_or(anyhow!("Path is not set".to_string()))?;

        if let Err(err) = fs::create_dir_all(directory) {
            warn!("Could not create db path {:?}", err);
        }

        let database = Database::create(path)?;

        let write_txn = database.begin_write()?;
        {
            let _ = write_txn.open_table(USERS)?;
            let _ = write_txn.open_table(PENDING)?;
            let _ = write_txn.open_table(PAID_FEES)?;
            let _ = write_txn.open_table(RECEIVED_FEES)?;
        }
        write_txn.commit()?;

        Ok(Self {
            db: Arc::new(Mutex::new(database)),
        })
    }

    pub async fn add_fee_paid(&self, payment_hash: &str, fee_msat: u64) -> Result<()> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;
        {
            let mut fee_table = write_txn.open_table(PAID_FEES)?;

            fee_table.insert(payment_hash, fee_msat)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub async fn add_fee_received(&self, payment_hash: &str, fee_msat: u64) -> Result<()> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;
        {
            let mut fee_table = write_txn.open_table(RECEIVED_FEES)?;

            fee_table.insert(payment_hash, fee_msat)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub async fn add_user(&self, username: &str, user: &UserKind) -> Result<()> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;
        {
            let mut users_table = write_txn.open_table(USERS)?;

            users_table.insert(username, user.as_json().as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub async fn get_user(&self, username: &str) -> Result<Option<UserKind>> {
        let db = self.db.lock().await;

        let read_txn = db.begin_read()?;
        let users_table = read_txn.open_table(USERS)?;

        let user = match users_table.get(username)? {
            Some(contact) => Some(serde_json::from_str(contact.value())?),
            None => None,
        };

        Ok(user)
    }

    pub async fn get_all_users(&self) -> Result<Vec<User>> {
        let db = self.db.lock().await;

        let read_txn = db.begin_read()?;
        let users_table = read_txn.open_table(USERS)?;

        let users = users_table
            .iter()?
            .flatten()
            .flat_map(|(_k, v)| serde_json::from_str(v.value()))
            .collect();
        Ok(users)
    }

    pub async fn get_pending_users(&self) -> Result<Vec<PendingUser>> {
        let db = self.db.lock().await;

        let read_txn = db.begin_read()?;
        let users_table = read_txn.open_table(USERS)?;

        let users = users_table
            .iter()?
            .flatten()
            .flat_map(|(_k, v)| match serde_json::from_str(v.value()) {
                Ok(UserKind::Pending(pending_user)) => Some(pending_user),
                _ => None,
            })
            .collect();
        Ok(users)
    }

    pub async fn delete_user(&self, username: &str) -> Result<()> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;
        {
            let mut users_table = write_txn.open_table(USERS)?;

            users_table.remove(username)?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub async fn add_pending_invoice(&self, hash: &str, invoice: &PendingInvoice) -> Result<()> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;
        {
            let mut pending_table = write_txn.open_table(PENDING)?;

            pending_table.insert(hash, invoice.as_json().as_str())?;
        }
        write_txn.commit()?;

        Ok(())
    }

    pub async fn get_pending_invoice(&self, hash: &str) -> Result<Option<PendingInvoice>> {
        let db = self.db.lock().await;

        let read_txn = db.begin_read()?;
        let pending_table = read_txn.open_table(PENDING)?;

        let user = match pending_table.get(hash)? {
            Some(contact) => Some(serde_json::from_str(contact.value())?),
            None => None,
        };

        Ok(user)
    }

    pub async fn get_pending_invoices(&self) -> Result<Vec<PendingInvoice>> {
        let db = self.db.lock().await;

        let read_txn = db.begin_read()?;
        let pending_table = read_txn.open_table(PENDING)?;

        let pending_invoices: Vec<PendingInvoice> =
            pending_table.iter()?.fold(Vec::new(), |mut vec, item| {
                if let Ok((_key, value)) = item {
                    if let Ok(pending) = serde_json::from_str(value.value()) {
                        vec.push(pending)
                    }
                }
                vec
            });

        Ok(pending_invoices)
    }

    pub async fn remove_pending_invoice(&self, hash: &str) -> Result<()> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;
        {
            let mut pending_table = write_txn.open_table(PENDING)?;

            pending_table.remove(hash)?;
        }
        write_txn.commit()?;

        Ok(())
    }
}
