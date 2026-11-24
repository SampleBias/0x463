use super::{db, *};
use settle463_core::{
    ident::timestamp,
    settlement::{Action, ActionReview, FundSignatures, OrderState, OrderView, SettlementBackend},
    wire::Body,
    SignedMessage,
};
use settle463_sim::Simulator;
use sqlx::Row;

fn action(v: &Value) -> Result<Action> {
    match field(v, "action")? {
        "commit_delivery" => Ok(Action::CommitDelivery),
        "release" => Ok(Action::Release),
        "refund" => Ok(Action::Refund),
        _ => Err(ApiError::bad("BAD_ENCODING")),
    }
}
async fn evidence(
    s: &State,
    db: &mut Conn,
    order: &str,
    act: Action,
    v: &Value,
) -> Result<Option<SignedMessage>> {
    let id = match act {
        Action::CommitDelivery => field(v, "delivery_id")?,
        Action::Release => {
            let id = field(v, "verdict_id")?;
            let sticky: Option<String> =
                sqlx::query_scalar("SELECT definitive_id FROM orders WHERE order_id=?")
                    .bind(order)
                    .fetch_one(&mut *db)
                    .await?;
            if sticky.as_deref() != Some(id) {
                return Err(ApiError::link());
            }
            id
        }
        Action::Refund => return Ok(None),
    };
    Ok(Some(db::load(db, id, &s.config.environment).await?))
}
async fn action_review(
    s: &State,
    db: &mut Conn,
    sim: &Simulator,
    order: &str,
    v: &Value,
) -> Result<(ActionReview, Option<SignedMessage>)> {
    let act = action(v)?;
    let evidence = evidence(s, db, order, act, v).await?;
    let review = sim.prepare_action(order, act, evidence.as_ref())?;
    Ok((review, evidence))
}
async fn execute(
    s: &State,
    db: &mut Conn,
    sim: &Simulator,
    order: &str,
    v: &Value,
) -> Result<String> {
    if field(v, "action")? == "fund" {
        let agreement = db::agreement(db, &s.config.environment, order).await?;
        let review = sim.prepare_fund(agreement)?;
        let signatures = FundSignatures {
            buyer: hex(field(v, "buyer_signature")?)?,
            seller: hex(field(v, "seller_signature")?)?,
        };
        Ok(sim.submit_fund(&review, &signatures)?.intent_id)
    } else {
        let (review, evidence) = action_review(s, db, sim, order, v).await?;
        let sig = v
            .get("signature")
            .map(|s| hex::<64>(s.as_str().unwrap_or("")))
            .transpose()?;
        Ok(sim
            .submit_action(&review, evidence.as_ref(), sig.as_ref())?
            .intent_id)
    }
}

/// The finalized journal and its execution timestamps reproduce the same simulator after restart.
/// The applying transaction atomically appends one entry, updates the projection and consumes its outbox item.
pub(crate) async fn restore(s: &State, db: &mut Conn) -> Result<Simulator> {
    let c = &s.config;
    let sim = Simulator::with_environment(
        &c.environment,
        c.clock,
        [(format!("sim:{}", c.buyer), c.opening_balance)],
    )?;
    let history: Vec<(String, Vec<u8>, String)> = sqlx::query_as(
        "SELECT order_id, signed_bytes, execution_clock FROM intents WHERE
         state='finalized_success' ORDER BY ledger_seq",
    )
    .fetch_all(&mut *db)
    .await?;
    for (order, bytes, clock) in history {
        sim.set_clock(timestamp(&clock)?)?;
        execute(s, db, &sim, &order, &serde_json::from_slice(&bytes)?).await?;
    }
    sim.set_clock(db::clock(db).await?)?;
    Ok(sim)
}
fn check_fields(command: &Value, signed: bool) -> Result<()> {
    let fields: &[&str] = match (field(command, "action")?, signed) {
        ("fund", true) => &["action", "buyer_signature", "seller_signature"],
        ("commit_delivery", true) => &["action", "delivery_id", "signature"],
        ("release", true) => &["action", "verdict_id", "signature"],
        ("commit_delivery", false) => &["action", "delivery_id"],
        ("release", false) => &["action", "verdict_id"],
        ("fund", false) | ("refund", _) => &["action"],
        _ => return Err(ApiError::bad("BAD_ENCODING")),
    };
    let keys = command.as_object().ok_or(ApiError::bad("BAD_ENCODING"))?;
    if keys.keys().any(|k| !fields.contains(&k.as_str())) {
        return Err(ApiError::bad("UNKNOWN_FIELD"));
    }
    for key in fields {
        field(command, key)?;
    }
    Ok(())
}

pub(crate) async fn prepare(s: &State, db: &mut Conn, order: &str, v: &Value) -> Result<Vec<u8>> {
    check_fields(v, false)?;
    let sim = restore(s, db).await?;
    if field(v, "action")? == "fund" {
        let a = db::agreement(db, &s.config.environment, order).await?;
        Ok(serde_json::to_vec(&sim.prepare_fund(a)?.review)?)
    } else {
        let (review, _) = action_review(s, db, &sim, order, v).await?;
        Ok(serde_json::to_vec(&review)?)
    }
}

pub(crate) async fn enqueue(s: &State, db: &mut Conn, order: &str, v: &Value) -> Result<String> {
    check_fields(v, true)?;
    let action = field(v, "action")?;
    let input = canonical::canonical(v)?;
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT intent_id FROM intents WHERE order_id=? AND action=? AND signed_bytes=?",
    )
    .bind(order)
    .bind(action)
    .bind(&input)
    .fetch_optional(&mut *db)
    .await?;
    if let Some(intent) = stored {
        return Ok(intent);
    }
    let sim = restore(s, db).await?;
    let intent = execute(s, db, &sim, order, v).await?;
    let now = http::now()?.to_string();
    let signature = v
        .get("signature")
        .or_else(|| v.get("buyer_signature"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO intents(intent_id, order_id, action, state, signed_bytes,
         local_signature) VALUES(?, ?, ?, 'prepared', ?, ?)",
    )
    .bind(&intent)
    .bind(order)
    .bind(action)
    .bind(input)
    .bind(signature)
    .execute(&mut *db)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(ApiError::conflict("STATE_CONFLICT"));
    }
    sqlx::query("INSERT INTO outbox(intent_id,available_at) VALUES(?,?)")
        .bind(&intent)
        .bind(now)
        .execute(&mut *db)
        .await?;
    db::audit(
        s,
        db,
        order,
        "intent_recorded",
        &json!({"intent_id":intent,"action":action}),
        vec![],
    )
    .await?;
    Ok(intent)
}
async fn receipt(s: &State, db: &mut Conn, view: &OrderView) -> Result<()> {
    let order = &view.review.order_id;
    let a = db::agreement(db, &s.config.environment, order).await?;
    let delivery = match &view.delivery_id {
        Some(id) => Some(db::load(db, id, &s.config.environment).await?),
        None => None,
    };
    let sticky: Option<String> =
        sqlx::query_scalar("SELECT definitive_id FROM orders WHERE order_id=?")
            .bind(order)
            .fetch_one(&mut *db)
            .await?;
    let verdict = match (&view.verdict_id, sticky) {
        (Some(id), _) => Some(db::load(db, id, &s.config.environment).await?),
        (None, Some(id)) if view.delivery_id.is_some() => {
            let m = db::load(db, &id, &s.config.environment).await?;
            if matches!(&m.unsigned().payload,Body::Verdict(v) if v.decision=="FAIL") {
                Some(m)
            } else {
                None
            }
        }
        _ => None,
    };
    let fail = verdict
        .as_ref()
        .filter(|m| matches!(&m.unsigned().payload,Body::Verdict(v) if v.decision=="FAIL"));
    let body = settle463_core::receipt::make_sim_receipt(view, fail)?;
    let receipt = db::issuer_sign(s, Body::Receipt(Box::new(body)))?;
    let id = hex_string(receipt.id());
    db::insert_message(db, &receipt, &receipt.to_wire()?).await?;
    db::audit(
        s,
        db,
        order,
        "receipt_issued",
        &json!({"receipt_id":id}),
        vec![id.clone()],
    )
    .await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT m.signed_bytes FROM audit_events e JOIN signed_messages m ON
         e.observation_id=m.message_id WHERE e.order_id=? ORDER BY e.seq",
    )
    .bind(order)
    .fetch_all(&mut *db)
    .await?;
    let observations = rows
        .iter()
        .map(|b| settle463_core::parse_signed(b, &s.config.environment))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let bundle = settle463_core::receipt::build_sim_bundle(
        &a,
        delivery.as_ref(),
        verdict.as_ref(),
        &receipt,
        &observations,
        view,
    )?;
    sqlx::query("INSERT INTO receipts VALUES(?,?,?)")
        .bind(order)
        .bind(id)
        .bind(bundle)
        .execute(db)
        .await?;
    Ok(())
}

pub(crate) async fn process(s: &State, id: &str) -> Result<()> {
    let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row =
        sqlx::query("SELECT order_id,action,signed_bytes,state FROM intents WHERE intent_id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if row.get::<String, _>("state").starts_with("finalized_") {
        return Ok(());
    }
    let order: String = row.get("order_id");
    let act: String = row.get("action");
    let bytes: Vec<u8> = row.get("signed_bytes");
    let v = serde_json::from_slice(&bytes)?;
    let sim = restore(s, &mut tx).await?;
    let version: i64 = sqlx::query_scalar("SELECT version FROM orders WHERE order_id=?")
        .bind(&order)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE intents SET state='submitted',attempts=attempts+1 WHERE intent_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    db::audit(
        s,
        &mut tx,
        &order,
        "submitted",
        &json!({"intent_id":id,"action":act}),
        vec![],
    )
    .await?;
    match execute(s, &mut tx, &sim, &order, &v).await {
        Ok(intent) => {
            if intent != id {
                return Err(ApiError::store());
            }
            let view = sim.inspect(&order)?.ok_or_else(ApiError::store)?;
            let changed = sqlx::query(
                "UPDATE orders SET state=?, held=?, paid=?, refunded=?, delivery_id=?, verdict_id=?,
                 terminal_locator=?, terminal_clock=?, version=version+1 WHERE order_id=? AND version=?",
            )
            .bind(format!("{:?}",view.state))
            .bind(view.held.to_string())
            .bind(view.paid.to_string())
            .bind(view.refunded.to_string())
            .bind(&view.delivery_id)
            .bind(&view.verdict_id)
            .bind(&view.terminal_locator)
            .bind(view.terminal_clock.map(|n|n.to_string()))
            .bind(&order)
            .bind(version)
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                return Err(ApiError::conflict("STATE_CONFLICT"));
            }
            sqlx::query(
                "UPDATE intents SET state='finalized_success', execution_clock=?, ledger_seq=(SELECT
                 coalesce(max(ledger_seq), 0)+1 FROM intents) WHERE intent_id=?",
            )
            .bind(sim.clock()?.to_string())
            .bind(id)
            .execute(&mut *tx)
            .await?;
            db::audit(
                s,
                &mut tx,
                &order,
                "finalized",
                &json!({"intent_id":id,"action":act}),
                vec![],
            )
            .await?;
            if matches!(view.state, OrderState::Paid | OrderState::Refunded) {
                receipt(s, &mut tx, &view).await?;
            }
        }
        Err(e) if e.0.is_server_error() => return Err(e),
        Err(_) => {
            sqlx::query("UPDATE intents SET state='finalized_failure' WHERE intent_id=?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    sqlx::query("DELETE FROM outbox WHERE intent_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result = status(&mut tx, &order, id).await?;
    sqlx::query("UPDATE idempotency SET response=? WHERE intent_id=? AND length(response)=0")
        .bind(result)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn recover(s: &State) -> Result<()> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT intent_id FROM outbox WHERE CAST(available_at AS INTEGER)<=? ORDER BY id",
    )
    .bind(http::now()?)
    .fetch_all(&s.pool)
    .await?;
    for id in ids {
        if let Err(error) = process(s, &id).await {
            eprintln!("intent {id} recovery: {error}");
            sqlx::query(
                "UPDATE outbox SET attempts=attempts+1,last_error=?,available_at=? WHERE intent_id=?",
            )
            .bind(error.1)
            .bind((http::now()? + 1).to_string())
            .bind(id)
            .execute(&s.pool)
            .await?;
        }
    }
    Ok(())
}
pub(crate) async fn status(db: &mut Conn, order: &str, id: &str) -> Result<Vec<u8>> {
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM intents WHERE intent_id=? AND order_id=?")
            .bind(id)
            .bind(order)
            .fetch_optional(db)
            .await?;
    let outcome = match state.as_deref() {
        Some("prepared" | "submitted") => "pending",
        Some(outcome) => outcome,
        None => "unknown",
    };
    json_bytes(json!({"intent_id":id,"outcome":outcome}))
}
