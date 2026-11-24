//! Fund a new simulator order using the worked example's public service and pricing terms.

use settle463_core::{
    parse_signed,
    settlement::{Agreement, FundSignatures},
    wire::Body,
};
use settle463_sdk::{
    BuyerClient, BuyerPolicy, Identity, MemoryTransport, QuoteTerms, RequestTerms, SellerClient,
    SellerPolicy, SettlementBackend,
};
use settle463_sim::{Simulator, ASSET_ID};
use std::{collections::BTreeSet, error::Error, sync::Arc};

fn main() -> Result<(), Box<dyn Error>> {
    let buyer_id = Identity::generate()?;
    let buyer_account = format!("sim:{}", buyer_id.public_hex());
    let seller_id = Identity::generate()?;
    let seller_key = seller_id.public_hex();
    let verifier = Identity::generate()?.public_hex();
    let receipt_issuer = Identity::generate()?.public_hex();
    let ledger = Arc::new(Simulator::new(
        1_700_000_000,
        [(buyer_account.clone(), 5_000_000)],
    )?);
    let env = ledger.environment().to_owned();
    let transport = Arc::new(MemoryTransport::new(&env)?);
    let verifiers = BTreeSet::from([verifier.clone()]);
    let environments = BTreeSet::from([env.clone()]);
    let buyer = BuyerClient::new(
        buyer_id,
        BuyerPolicy {
            trusted_publishers: BTreeSet::from([seller_key]),
            max_principal: 2_500_000,
            allowed_services: BTreeSet::from(["sample-json-report".into()]),
            allowed_assets: BTreeSet::from([ASSET_ID.into()]),
            allowed_environments: environments.clone(),
            allowed_verifiers: verifiers.clone(),
            max_deliver_window_seconds: 86_400,
            max_resolve_window_seconds: 86_400,
        },
        transport.clone(),
        ledger.clone(),
    );
    let seller = SellerClient::new(
        seller_id,
        SellerPolicy {
            min_principal: 1_000_000,
            allowed_verifiers: verifiers,
            allowed_environments: environments,
        },
        transport,
        ledger.clone(),
    );

    let offer = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/service_offer.json"),
        "sim:fixture",
    )?;
    let Body::ServiceOffer(mut descriptor) = offer.unsigned().payload.clone() else {
        return Err("expected offer".into());
    };
    descriptor.environments = vec![env];
    seller.publish_offer(descriptor)?;
    let trusted = buyer.discover()?.pop().ok_or("missing trusted offer")?;
    let example = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/quote.json"),
        "sim:fixture",
    )?;
    let Body::Quote(q) = example.unsigned().payload.clone() else {
        return Err("expected quote".into());
    };
    let request = buyer.create_request(
        &trusted,
        RequestTerms {
            asset: q.asset,
            max_principal: 2_500_000,
            allowed_verifiers: vec![verifier.clone()],
            max_bytes: 1_048_576,
            expires_at: 1_700_003_600,
        },
    )?;
    let quote = seller.quote(
        &request,
        QuoteTerms {
            amount: 2_000_000,
            verifier,
            receipt_issuer,
            policy: q.acceptance_policy,
            deadlines: q.deadlines,
            expires_at: 1_700_003_600,
            fees: q.fees,
            artifact_store: q.artifact_store,
        },
    )?;
    let acceptance = buyer.accept(&request, &quote)?;
    let fund = ledger.prepare_fund(Agreement {
        request,
        quote,
        acceptance,
    })?;
    let signatures = FundSignatures {
        buyer: buyer.sign_fund(&fund)?,
        seller: seller.sign_fund(&fund)?,
    };
    ledger.submit_fund(&fund, &signatures)?;
    let order = buyer
        .inspect(&fund.review.order_id)?
        .ok_or("missing funded order")?;
    println!(
        "{:?} held={} paid={} refunded={} buyer_balance={} seller_balance={}",
        order.state,
        order.held,
        order.paid,
        order.refunded,
        ledger.balance(&buyer_account, ASSET_ID)?,
        ledger.balance(&fund.review.seller_destination, ASSET_ID)?
    );
    Ok(())
}
