use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "A service to dm cashu tokens for lnurl address", author = env!("CARGO_PKG_AUTHORS"), version = env!("CARGO_PKG_VERSION"))]
pub struct CLIArgs {
    #[arg(help = "path to config file", required = false)]
    pub config: Option<String>,
    #[arg(short, long, help = "Url of service", required = false)]
    pub url: Option<String>,
    #[arg(short, long, help = "Default Mint", required = false)]
    pub mint: Option<String>,
    #[arg(short, long, help = "Default Invice Description", required = false)]
    pub invoice_description: Option<String>,
    #[arg(short, long, help = "Nostr Nsec to send dms", required = false)]
    pub nsec: Option<String>,
    #[arg(
        short,
        long,
        help = "Default Nostr realys to publish",
        action = clap::ArgAction::Append, required = false
    )]
    pub relays: Vec<String>,
    #[arg(short, long, help = "Path to Database", required = false)]
    pub db_path: Option<String>,
    #[arg(long, help = "Whether or not to proxy ln invoice", required = false)]
    pub proxy: Option<bool>,
    #[arg(
        short,
        long,
        help = "Fee to collect to account for routing fee percent as a decimal",
        required = false
    )]
    pub fee: Option<f32>,
    #[arg(long, help = "cln path", required = false)]
    pub cln_path: Option<String>,
    #[arg(long, help = "Min Sendable in sats", required = false)]
    pub min_sendable: Option<u64>,
    #[arg(long, help = "Max Sendable in sats", required = false)]
    pub max_sendable: Option<u64>,
    #[arg(short, long, help = "Publish Zaps", required = false)]
    pub zapper: Option<bool>,
    #[arg(long, help = "Pay index path", required = false)]
    pub pay_index_path: Option<PathBuf>,
    #[arg(short, long, help = "Network address to bind", required = false)]
    pub address: Option<String>,
    #[arg(long, help = "Network port to bind", required = false)]
    pub port: Option<u16>,
    #[arg(long, help = "Price for two char or less username", required = false)]
    pub two_char_price: Option<u64>,
    #[arg(long, help = "Price for three char username", required = false)]
    pub three_char_price: Option<u64>,
    #[arg(long, help = "Price for 4 char username", required = false)]
    pub four_char_price: Option<u64>,
    #[arg(long, help = "Price for 5+ char username", required = false)]
    pub other_char_price: Option<u64>,
}
