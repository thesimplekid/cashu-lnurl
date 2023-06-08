use cashu_crab::Amount;

pub fn amount_from_msat(msats: u64) -> Amount {
    let sats = msats / 1000;
    Amount::from_sat(sats)
}

pub fn msat_from_amount(amount: &Amount) -> u64 {
    amount.to_sat() * 1000
}
