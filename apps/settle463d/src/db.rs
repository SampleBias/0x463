use super::*;
use settle463_core::{
    check_quote_request,
    ident::{decimal, timestamp},
    parse_signed,
    settlement::{Agreement, Review},
    sign,
    wire::{Body, Observation},
    SignedMessage, UnsignedMessage,
};
use sqlx::Row;

pub(crate) async fn clock(db: &mut Conn) -> Result<i64> {
    let value: String = sqlx::query_scalar("SELECT clock FROM deployment")
        .fetch_one(db)
        .await?;
    Ok(timestamp(&value)?)
}
pub(crate) async fn load(db: &mut Conn, id: &str, env: &str) -> Result<SignedMessage> {
    let bytes: Vec<u8> =
        sqlx::query_scalar("SELECT signed_bytes FROM signed_messages WHERE message_id=?")
            .bind(id)
            .fetch_one(db)
            .await?;
    Ok(parse_signed(&bytes, env)?)
}
pub(crate) async fn raw(db: &mut Conn, id: &str) -> Result<Vec<u8>> {
    Ok(
        sqlx::query_scalar("SELECT signed_bytes FROM signed_messages WHERE message_id=?")
            .bind(id)
            .fetch_one(db)
            .await?,
    )
}
pub(crate) async fn value(db: &mut Conn, id: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&raw(db, id).await?)?)
}
pub(crate) async fn agreement(db: &mut Conn, env: &str, order: &str) -> Result<Agreement> {
    let (r, q, a): (String, String, String) =
        sqlx::query_as("SELECT request_id,quote_id,acceptance_id FROM orders WHERE order_id=?")
            .bind(order)
            .fetch_one(&mut *db)
            .await?;
    Ok(Agreement {
        request: load(db, &r, env).await?,
        quote: load(db, &q, env).await?,
        acceptance: load(db, &a, env).await?,
    })
}

fn order_of(m: &SignedMessage) -> Option<&str> {
    match &m.unsigned().payload {
        Body::ServiceOffer(_) => None,
        Body::Request(x) => Some(&x.order_id),
        Body::Quote(x) => Some(&x.order_id),
        Body::Accept(x) => Some(&x.order_id),
        Body::Delivery(x) => Some(&x.order_id),
        Body::Verdict(x) => Some(&x.order_id),
        Body::Receipt(x) => Some(&x.order_id),
        Body::Observation(x) => Some(&x.order_id),
    }
}
pub(crate) fn kind(m: &SignedMessage) -> &'static str {
    match &m.unsigned().payload {
        Body::ServiceOffer(_) => "service_offer",
        Body::Request(_) => "request",
        Body::Quote(_) => "quote",
        Body::Accept(_) => "accept",
        Body::Delivery(_) => "delivery",
        Body::Verdict(_) => "verdict",
        Body::Receipt(_) => "receipt",
        Body::Observation(_) => "observation",
    }
}
pub(crate) async fn insert_message(db: &mut Conn, m: &SignedMessage, bytes: &[u8]) -> Result<()> {
    let service = if let Body::ServiceOffer(o) = &m.unsigned().payload {
        Some(&o.service_id)
    } else {
        None
    };
    sqlx::query("INSERT INTO signed_messages(message_id,kind,environment,issuer,signed_bytes,canonical,signature,order_id,service_id) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(hex_string(m.id())).bind(kind(m)).bind(&m.unsigned().environment).bind(&m.unsigned().issuer).bind(bytes)
        .bind(canonical::canonical(&serde_json::to_value(m.unsigned())?)?).bind(hex_string(m.signature())).bind(order_of(m))
        .bind(service).execute(db).await?;
    Ok(())
}
pub(crate) fn issuer_sign(s: &State, body: Body) -> Result<SignedMessage> {
    Ok(sign(
        UnsignedMessage {
            protocol: "0x463".into(),
            version: "0.1".into(),
            environment: s.config.environment.clone(),
            issuer: hex_string(s.key.verifying_key().as_bytes()),
            payload: body,
        },
        &s.key,
    )?)
}
pub(crate) async fn audit(
    s: &State,
    db: &mut Conn,
    order: &str,
    event: &str,
    payload: &Value,
    refs: Vec<String>,
) -> Result<()> {
    let prev: Option<(i64, String)> = sqlx::query_as(
        "SELECT seq,event_digest FROM audit_events WHERE order_id=? ORDER BY seq DESC LIMIT 1",
    )
    .bind(order)
    .fetch_optional(&mut *db)
    .await?;
    let (last, previous) = prev.unwrap_or((0, "00".repeat(32)));
    let seq = last.checked_add(1).ok_or_else(ApiError::store)?;
    let payload_hash = digest(&canonical::canonical(payload)?);
    let h = digest(&preimage(
        b"0x463/event/0.1",
        &json!({"event_type":event,"order_id":order,"payload_hash":payload_hash,
        "prev_digest":previous,"seq":seq.to_string()}),
    )?);
    let observation = issuer_sign(
        s,
        Body::Observation(Observation {
            order_id: order.to_owned(),
            event_seq: seq.to_string(),
            event_digest: h.clone(),
            event_type: event.to_owned(),
            refs,
        }),
    )?;
    insert_message(db, &observation, &observation.to_wire()?).await?;
    sqlx::query("INSERT INTO audit_events VALUES(?,?,?,?,?,?,?)")
        .bind(order)
        .bind(seq)
        .bind(event)
        .bind(payload_hash)
        .bind(previous)
        .bind(h)
        .bind(hex_string(observation.id()))
        .execute(db)
        .await?;
    Ok(())
}

pub(crate) async fn member(db: &mut Conn, order: &str, key: &str, role: &str) -> Result<bool> {
    let role_field = match role {
        "buyer" => "buyer",
        "seller" => "seller",
        "verifier" => "verifier",
        "receipt_issuer" => "receipt_issuer",
        _ => return Ok(false),
    };
    let stmt = format!("SELECT {role_field} FROM orders WHERE order_id=?");
    let owner: Option<String> = sqlx::query_scalar(&stmt)
        .bind(order)
        .fetch_optional(db)
        .await?;
    Ok(owner.as_deref() == Some(key))
}
pub(crate) async fn publish(
    s: &State,
    db: &mut Conn,
    msg: &SignedMessage,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    let env = &s.config.environment;
    let id = hex_string(msg.id());
    let exists: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT signed_bytes FROM signed_messages WHERE message_id=?")
            .bind(&id)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(prior) = exists {
        if matches!(msg.unsigned().payload, Body::Accept(_)) {
            return Err(ApiError::conflict("ACCEPTANCE_EXISTS"));
        }
        if let Body::Verdict(v) = &msg.unsigned().payload {
            let sticky: Option<String> =
                sqlx::query_scalar("SELECT definitive_id FROM orders WHERE order_id=?")
                    .bind(&v.order_id)
                    .fetch_one(&mut *db)
                    .await?;
            if let Some(sticky) = sticky {
                return raw(db, &sticky).await;
            }
        }
        return Ok(prior);
    }
    match &msg.unsigned().payload {
        Body::ServiceOffer(_) => insert_message(db, msg, bytes).await?,
        Body::Request(r) => {
            let offer = load(db, &r.offer_id, env).await?;
            let Body::ServiceOffer(o) = &offer.unsigned().payload else {
                return Err(ApiError::link());
            };
            if offer.unsigned().issuer != r.seller
                || o.service_id != r.service_id
                || !o.environments.contains(env)
                || !o
                    .verification_profiles
                    .contains(&r.artifact_constraints.verification_profile)
                || decimal(&r.artifact_constraints.max_bytes)?
                    > decimal(&o.input_profile.max_bytes)?
            {
                return Err(ApiError::link());
            }
            insert_message(db, msg, bytes).await?;
            audit(
                s,
                db,
                &r.order_id,
                "request_stored",
                &json!({"request_id":id}),
                vec![id],
            )
            .await?;
        }
        Body::Quote(q) => {
            let request = load(db, &q.request_id, env).await?;
            check_quote_request(msg, &request).map_err(|_| ApiError::link())?;
            if q.receipt_issuer != hex_string(s.key.verifying_key().as_bytes()) {
                return Err(ApiError::link());
            }
            insert_message(db, msg, bytes).await?;
            audit(
                s,
                db,
                &q.order_id,
                "quote_stored",
                &json!({"quote_id":id}),
                vec![id],
            )
            .await?;
        }
        Body::Accept(a) => {
            let agreement = Agreement {
                request: load(db, &a.request_id, env).await?,
                quote: load(db, &a.quote_id, env).await?,
                acceptance: msg.clone(),
            };
            let (r, q) = agreement.terms(env)?;
            let review = Review::sim_fund(&agreement, env)?;
            if clock(db).await? >= timestamp(&q.deadlines.fund_by)? {
                return Err(ApiError::conflict("STATE_CONFLICT"));
            }
            let n = sqlx::query("INSERT OR IGNORE INTO acceptances VALUES(?,?,?,?,?)")
                .bind(env)
                .bind(&r.buyer)
                .bind(&r.nonce)
                .bind(&a.request_id)
                .bind(&a.order_id)
                .execute(&mut *db)
                .await?
                .rows_affected();
            if n == 0 {
                return Err(ApiError::conflict("ACCEPTANCE_EXISTS"));
            }
            insert_message(db, msg, bytes).await?;
            sqlx::query("INSERT INTO orders(order_id,request_id,quote_id,acceptance_id,buyer,seller,verifier,receipt_issuer,principal,buyer_destination,seller_destination,fund_by,deliver_by,resolve_by,state) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,'Accepted')")
             .bind(&a.order_id).bind(&a.request_id).bind(&a.quote_id).bind(&id).bind(&q.buyer).bind(&q.seller).bind(&q.verifier).bind(&q.receipt_issuer)
             .bind(&q.amount).bind(&review.buyer_destination).bind(&review.seller_destination).bind(&q.deadlines.fund_by).bind(&q.deadlines.deliver_by).bind(&q.deadlines.resolve_by).execute(&mut *db).await?;
            audit(
                s,
                db,
                &a.order_id,
                "accepted",
                &json!({"acceptance_id":id}),
                vec![id],
            )
            .await?;
        }
        Body::Delivery(d) => {
            let agreement = agreement(db, env, &d.order_id).await?;
            let review = Review::sim_fund(&agreement, env)?;
            settle463_core::settlement::check_delivery(&review, msg)?;
            let row: Option<(String, String)> = sqlx::query_as(
                "SELECT digest,length FROM artifacts WHERE content_id=? AND order_id=?",
            )
            .bind(&d.content_id)
            .bind(&d.order_id)
            .fetch_optional(&mut *db)
            .await?;
            if row != Some((d.digest.clone(), d.length.clone())) {
                return Err(ApiError::link());
            }
            insert_message(db, msg, bytes).await?;
        }
        Body::Verdict(v) => {
            let row =
                sqlx::query("SELECT delivery_id,definitive_id,state FROM orders WHERE order_id=?")
                    .bind(&v.order_id)
                    .fetch_one(&mut *db)
                    .await?;
            let committed: Option<String> = row.get("delivery_id");
            if committed.as_deref() != Some(&v.delivery_id) {
                return Err(ApiError::link());
            }
            let a = agreement(db, env, &v.order_id).await?;
            let review = Review::sim_fund(&a, env)?;
            let delivery = load(db, &v.delivery_id, env).await?;
            settle463_core::settlement::check_verdict(&review, &delivery, msg)?;
            let (_, q) = a.terms(env)?;
            if v.decision == "PASS" {
                settle463_core::settlement::check_pass_policy(&q.artifact, msg)?;
            }
            if let Some(previous) = row.get::<Option<String>, _>("definitive_id") {
                return raw(db, &previous).await;
            }
            if row.get::<String, _>("state") != "Delivered" {
                return Err(ApiError::conflict("STATE_CONFLICT"));
            }
            if v.decision != "INDETERMINATE"
                && clock(db).await? >= timestamp(&q.deadlines.resolve_by)?
            {
                return Err(ApiError::conflict("STATE_CONFLICT"));
            }
            insert_message(db, msg, bytes).await?;
            if v.decision != "INDETERMINATE" {
                sqlx::query("UPDATE orders SET definitive_id=?,version=version+1 WHERE order_id=? AND definitive_id IS NULL")
                  .bind(&id).bind(&v.order_id).execute(&mut *db).await?;
            }
        }
        Body::Receipt(_) | Body::Observation(_) => return Err(ApiError::denied("ROLE_FORBIDDEN")),
    }
    Ok(bytes.to_vec())
}

pub(crate) async fn order_projection(s: &State, db: &mut Conn, order: &str) -> Result<Vec<u8>> {
    let row = sqlx::query("SELECT * FROM orders WHERE order_id=?")
        .bind(order)
        .fetch_one(&mut *db)
        .await?;
    let agreement = agreement(db, &s.config.environment, order).await?;
    let review = Review::sim_fund(&agreement, &s.config.environment)?;
    let sim = actions::restore(s, db).await?;
    let sticky: Option<String> = row.get("definitive_id");
    let verdict = if let Some(id) = sticky {
        Some(value(db, &id).await?)
    } else {
        None
    };
    json_bytes(
        json!({"order_id":order,"state":row.get::<String,_>("state"),"held":row.get::<String,_>("held"),"paid":row.get::<String,_>("paid"),"refunded":row.get::<String,_>("refunded"),
        "review":review,"request":value(db,&review.request_id).await?,"quote":value(db,&review.quote_id).await?,"acceptance":value(db,&review.acceptance_id).await?,
        "definitive_verdict":verdict,"delivery_id":row.get::<Option<String>,_>("delivery_id"),"verdict_id":row.get::<Option<String>,_>("verdict_id"),
        "terminal_clock":row.get::<Option<String>,_>("terminal_clock"),"terminal_locator":row.get::<Option<String>,_>("terminal_locator"),
        "buyer_balance":sim.balance(&review.buyer_destination,&review.asset.id)?.to_string(),"seller_balance":sim.balance(&review.seller_destination,&review.asset.id)?.to_string(),"clock":clock(db).await?.to_string()}),
    )
}
