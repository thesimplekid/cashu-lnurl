use anyhow::Result;
use log::debug;
use redb::{Database, ReadableTable, TableDefinition};
use std::{fs, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

use crate::types::{PendingInvoice, User};

const USERS: TableDefinition<&str, &str> = TableDefinition::new("mint_info");

const PENDING: TableDefinition<&str, &str> = TableDefinition::new("pending");

#[derive(Debug, Clone)]
pub struct Db {
    db: Arc<Mutex<Database>>,
}

impl Db {
    /// Init Database
    pub async fn new(path: PathBuf) -> Result<Self> {
        if let Err(_err) = fs::create_dir_all(&path) {}
        let db_path = path.join("cashu-lnurl.redb");
        let database = Database::create(db_path)?;

        let write_txn = database.begin_write()?;
        {
            let _ = write_txn.open_table(USERS)?;
            let _ = write_txn.open_table(PENDING)?;
        }
        write_txn.commit()?;

        Ok(Self {
            db: Arc::new(Mutex::new(database)),
        })
    }

    pub async fn add_user(&self, username: &str, user: &User) -> Result<()> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write()?;
        {
            let mut users_table = write_txn.open_table(USERS)?;

            users_table.insert(username, user.as_json().as_str())?;
        }
        write_txn.commit()?;

        debug!("User: {username}, {:?} added", user);

        Ok(())
    }

    pub async fn get_user(&self, username: &str) -> Result<Option<User>> {
        let db = self.db.lock().await;

        let read_txn = db.begin_read()?;
        let users_table = read_txn.open_table(USERS)?;

        let user = match users_table.get(username)? {
            Some(contact) => Some(serde_json::from_str(contact.value())?),
            None => None,
        };

        Ok(user)
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

    pub async fn _get_pending_invoice(&self, hash: &str) -> Result<Option<PendingInvoice>> {
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
