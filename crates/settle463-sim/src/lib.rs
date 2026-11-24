//! In-memory settlement ledger with a controllable clock and atomic funding.

use settle463_core::ident::{decimal, hex, hex_string, timestamp};
use settle463_core::settlement::{
    Agreement, ChainOutcome, FundSignatures, FundingError, OrderState, OrderView, PreparedFund,
    Result, Review, SettlementBackend, Submission,
};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

pub const ASSET_ID: &str = "test-six-decimal";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub intent_id: String,
    pub order_id: String,
    pub asset_id: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
}

struct Ledger {
    clock: i64,
    manual: bool,
    balances: HashMap<(String, String), u64>,
    orders: HashMap<String, OrderView>,
    transfers: Vec<Transfer>,
    pending: HashMap<String, Review>,
    pending_orders: HashMap<String, String>,
    outcomes: HashMap<String, ChainOutcome>,
}

pub struct Simulator {
    environment: String,
    ledger: Mutex<Ledger>,
}

impl Simulator {
    /// Creates an isolated deployment. Opening accounts must be unique party accounts.
    pub fn new(clock: i64, opening: impl IntoIterator<Item = (String, u64)>) -> Result<Self> {
        if clock < 0 {
            return Err(FundingError::InvalidGenesis);
        }
        let mut balances = HashMap::new();
        for (account, amount) in opening {
            let key = account
                .strip_prefix("sim:")
                .ok_or(FundingError::InvalidGenesis)?;
            hex::<32>(key)?;
            if balances
                .insert((account, ASSET_ID.to_owned()), amount)
                .is_some()
            {
                return Err(FundingError::InvalidGenesis);
            }
        }
        let mut id = [0u8; 16];
        getrandom::getrandom(&mut id).map_err(|_| FundingError::Unavailable)?;
        Ok(Self {
            environment: format!("sim:{}", hex_string(&id)),
            ledger: Mutex::new(Ledger {
                clock,
                manual: false,
                balances,
                orders: HashMap::new(),
                transfers: Vec::new(),
                pending: HashMap::new(),
                pending_orders: HashMap::new(),
                outcomes: HashMap::new(),
            }),
        })
    }
    fn ledger(&self) -> Result<MutexGuard<'_, Ledger>> {
        self.ledger.lock().map_err(|_| FundingError::Unavailable)
    }
    /// Advances ledger time. A deployment cannot rewind a deadline.
    pub fn set_clock(&self, seconds: i64) -> Result<()> {
        let mut ledger = self.ledger()?;
        if seconds < ledger.clock {
            return Err(FundingError::Deadline);
        }
        ledger.clock = seconds;
        Ok(())
    }
    pub fn set_manual_finalization(&self, manual: bool) -> Result<()> {
        self.ledger()?.manual = manual;
        Ok(())
    }
    pub fn balance(&self, account: &str, asset_id: &str) -> Result<u64> {
        Ok(self.ledger()?.balance(account, asset_id))
    }
    pub fn transfers(&self) -> Result<Vec<Transfer>> {
        Ok(self.ledger()?.transfers.clone())
    }

    /// Executes a previously submitted command against the current time and balances.
    pub fn finalize(&self, intent_id: &str) -> Result<ChainOutcome> {
        let mut ledger = self.ledger()?;
        let Some(review) = ledger.pending.remove(intent_id) else {
            return Ok(ledger.outcome(intent_id));
        };
        ledger.pending_orders.remove(&review.order_id);
        let outcome = match ledger.apply(&review, intent_id) {
            Ok(()) => ChainOutcome::FinalizedSuccess,
            Err(FundingError::Deadline) => ChainOutcome::Expired,
            Err(_) => ChainOutcome::FinalizedFailure,
        };
        ledger.outcomes.insert(intent_id.to_owned(), outcome);
        Ok(outcome)
    }
}

impl Ledger {
    fn balance(&self, account: &str, asset: &str) -> u64 {
        self.balances
            .get(&(account.to_owned(), asset.to_owned()))
            .copied()
            .unwrap_or(0)
    }
    fn outcome(&self, intent: &str) -> ChainOutcome {
        self.outcomes
            .get(intent)
            .copied()
            .unwrap_or(ChainOutcome::Unknown)
    }
    fn check(&self, r: &Review) -> Result<(u64, u64)> {
        if self.orders.contains_key(&r.order_id) {
            return Err(FundingError::AlreadyFunded);
        }
        if self.clock >= timestamp(&r.deadlines.fund_by)? {
            return Err(FundingError::Deadline);
        }
        let amount = decimal(&r.principal)?;
        let remainder = self
            .balance(&r.buyer_destination, &r.asset.id)
            .checked_sub(amount)
            .ok_or(FundingError::InsufficientFunds)?;
        Ok((amount, remainder))
    }
    fn apply(&mut self, r: &Review, id: &str) -> Result<()> {
        let (amount, remainder) = self.check(r)?;
        // Only this order can create its hold account, and its order ID can fund once.
        self.balances
            .insert((r.buyer_destination.clone(), r.asset.id.clone()), remainder);
        self.balances
            .insert((r.escrow_account.clone(), r.asset.id.clone()), amount);
        self.orders.insert(
            r.order_id.clone(),
            OrderView {
                state: OrderState::Funded,
                review: r.clone(),
                held: amount,
                paid: 0,
                refunded: 0,
            },
        );
        self.transfers.push(Transfer {
            intent_id: id.to_owned(),
            order_id: r.order_id.clone(),
            asset_id: r.asset.id.clone(),
            from: r.buyer_destination.clone(),
            to: r.escrow_account.clone(),
            amount,
        });
        Ok(())
    }
}

impl SettlementBackend for Simulator {
    fn environment(&self) -> &str {
        &self.environment
    }
    fn clock(&self) -> Result<i64> {
        Ok(self.ledger()?.clock)
    }
    fn prepare_fund(&self, agreement: Agreement) -> Result<PreparedFund> {
        let review = Review::sim_fund(&agreement, self.environment())?;
        self.ledger()?.check(&review)?;
        Ok(PreparedFund { agreement, review })
    }
    fn submit_fund(&self, fund: &PreparedFund, signatures: &FundSignatures) -> Result<Submission> {
        fund.check(self.environment())?;
        let review = &fund.review;
        let intent_id = signatures.verify(review)?;
        let mut ledger = self.ledger()?;
        if ledger.pending_orders.contains_key(&review.order_id) {
            return Err(FundingError::Pending);
        }
        let outcome = if ledger.manual {
            ledger.check(review)?;
            ledger
                .pending_orders
                .insert(review.order_id.clone(), intent_id.clone());
            ledger.pending.insert(intent_id.clone(), review.clone());
            ChainOutcome::Pending
        } else {
            ledger.apply(review, &intent_id)?;
            ChainOutcome::FinalizedSuccess
        };
        ledger.outcomes.insert(intent_id.clone(), outcome);
        Ok(Submission { intent_id, outcome })
    }
    fn inspect(&self, order_id: &str) -> Result<Option<OrderView>> {
        Ok(self.ledger()?.orders.get(order_id).cloned())
    }
    fn reconcile(&self, intent_id: &str) -> Result<ChainOutcome> {
        Ok(self.ledger()?.outcome(intent_id))
    }
}
