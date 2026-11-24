use super::*;
use settle463_core::settlement::{
    Action, FundingError, OrderView, Result as DomainResult, SettlementBackend, Submission,
};

/// The coordinator commits the signed transaction and its outbox item atomically.
/// A retry returns the selected attempt so concurrent callers broadcast identical bytes.
pub trait IntentJournal: Send + Sync {
    fn stage(&self, order: &str, action: ChainAction, signed: &SignedTx) -> Result<SignedTx>;
    fn find(&self, order: &str, signature: &str) -> Result<Option<(ChainAction, SignedTx)>>;
}

#[derive(Clone)]
pub struct NativeReview {
    pub expected: Expected,
    pub prepared: Prepared,
}

/// An order-scoped adapter for applications supplying durable intent storage.
pub struct OrderBackend<J> {
    rpc: Rpc,
    agreement: Agreement,
    delivery: Option<SignedMessage>,
    verdict: Option<SignedMessage>,
    payer: String,
    environment: String,
    journal: J,
}

fn domain(e: Error) -> FundingError {
    match e {
        Error::Invalid(_) => FundingError::Linkage,
        Error::Unavailable(_) => FundingError::Unavailable,
    }
}
impl<J: IntentJournal> OrderBackend<J> {
    pub fn new(
        rpc: Rpc,
        agreement: Agreement,
        delivery: Option<SignedMessage>,
        verdict: Option<SignedMessage>,
        payer: String,
        journal: J,
    ) -> Result<Self> {
        let environment = rpc.deployment.environment();
        agreement.terms(&environment).map_err(invalid)?;
        key(&payer)?;
        Ok(Self {
            rpc,
            agreement,
            delivery,
            verdict,
            payer,
            environment,
            journal,
        })
    }
    fn expected(
        &self,
        action: ChainAction,
        evidence: Option<&SignedMessage>,
        payer: &str,
    ) -> Result<Expected> {
        let (delivery, verdict) = match action {
            ChainAction::Fund => (None, None),
            ChainAction::CommitDelivery => (evidence.or(self.delivery.as_ref()), None),
            ChainAction::Release => (self.delivery.as_ref(), evidence.or(self.verdict.as_ref())),
            ChainAction::Refund => (self.delivery.as_ref(), None),
        };
        Expected::new(
            &self.rpc.deployment,
            &self.agreement,
            action,
            delivery,
            verdict,
            payer,
        )
    }
    fn prepare(&self, e: Expected) -> DomainResult<NativeReview> {
        let prepared = self.rpc.prepare(&e).map_err(domain)?;
        Ok(NativeReview {
            expected: e,
            prepared,
        })
    }
    fn submit(&self, e: Expected, p: &Prepared, signatures: &[String]) -> DomainResult<Submission> {
        let signed = e.assemble(p, signatures).map_err(domain)?;
        let selected = self
            .journal
            .stage(&e.review().order_id, e.action, &signed)
            .map_err(domain)?;
        e.validate_signed(&selected).map_err(domain)?;
        match self.rpc.broadcast(&selected) {
            Ok(()) | Err(Error::Unavailable(_)) => (),
            Err(err) => return Err(domain(err)),
        }
        let outcome = match self.rpc.reconcile(&e, &selected.signature, Some(&selected)) {
            Ok(r) => r.outcome,
            Err(Error::Unavailable(_)) => ChainOutcome::Unknown,
            Err(err) => return Err(domain(err)),
        };
        Ok(Submission {
            intent_id: selected.signature,
            outcome,
        })
    }
}
impl<J: IntentJournal> SettlementBackend<NativeReview, NativeReview, Vec<String>, Vec<String>>
    for OrderBackend<J>
{
    fn environment(&self) -> &str {
        &self.environment
    }
    fn clock(&self) -> DomainResult<i64> {
        self.rpc.clock().map_err(domain)
    }
    fn prepare_fund(&self, agreement: Agreement) -> DomainResult<NativeReview> {
        let e = Expected::new(
            &self.rpc.deployment,
            &agreement,
            ChainAction::Fund,
            None,
            None,
            &self.payer,
        )
        .map_err(domain)?;
        if self
            .expected(ChainAction::Fund, None, &self.payer)
            .map_err(domain)?
            .terms
            != e.terms
        {
            return Err(FundingError::Linkage);
        }
        self.prepare(e)
    }
    fn submit_fund(&self, review: &NativeReview, sigs: &Vec<String>) -> DomainResult<Submission> {
        self.submit(
            self.expected(ChainAction::Fund, None, &self.payer)
                .map_err(domain)?,
            &review.prepared,
            sigs,
        )
    }
    fn prepare_action(
        &self,
        order: &str,
        action: Action,
        evidence: Option<&SignedMessage>,
    ) -> DomainResult<NativeReview> {
        let action = match action {
            Action::CommitDelivery => ChainAction::CommitDelivery,
            Action::Release => ChainAction::Release,
            Action::Refund => ChainAction::Refund,
        };
        let e = self
            .expected(action, evidence, &self.payer)
            .map_err(domain)?;
        if order != e.terms.order_id {
            return Err(FundingError::OrderMissing);
        }
        self.prepare(e)
    }
    fn submit_action(
        &self,
        review: &NativeReview,
        evidence: Option<&SignedMessage>,
        signature: Option<&Vec<String>>,
    ) -> DomainResult<Submission> {
        if review.expected.action == ChainAction::Fund {
            return Err(FundingError::Linkage);
        }
        let e = self
            .expected(review.expected.action, evidence, &self.payer)
            .map_err(domain)?;
        self.submit(e, &review.prepared, signature.ok_or(FundingError::Linkage)?)
    }
    fn inspect(&self, order: &str) -> DomainResult<Option<OrderView>> {
        let e = Expected::new(
            &self.rpc.deployment,
            &self.agreement,
            ChainAction::Fund,
            None,
            None,
            "",
        )
        .map_err(domain)?;
        if order != e.terms.order_id {
            return Ok(None);
        }
        self.rpc.inspect(&e).map_err(domain)
    }
    fn reconcile(&self, id: &str) -> DomainResult<ChainOutcome> {
        let (_, q) = self.agreement.terms(&self.environment)?;
        let Some((action, signed)) = self.journal.find(&q.order_id, id).map_err(domain)? else {
            return Ok(ChainOutcome::Unknown);
        };
        let e = self.expected(action, None, "").map_err(domain)?;
        self.rpc
            .reconcile(&e, id, Some(&signed))
            .map(|r| r.outcome)
            .map_err(domain)
    }
}
