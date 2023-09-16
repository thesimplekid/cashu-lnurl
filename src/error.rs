// use cashu_crab::error::Error as CashuCrabError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cashu Crab Error: {0}")]
    CashuError(#[from] cashu_sdk::error::Error),
    #[error("Cashu Crab Client Error: {0}")]
    CashuCrabClient(#[from] cashu_sdk::client::Error),
}
