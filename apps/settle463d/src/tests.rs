use super::*;
use axum::{
    body::{to_bytes, Body as HttpBody},
    extract::{Request, State as AxumState},
};
use ed25519_dalek::Signer;
use settle463_core::{
    order_id, parse_signed,
    settlement::{ActionReview, Review},
    sign,
    wire::{Body, Settlement},
    SignedMessage,
};

const OFFER: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/service_offer.json");
const REQUEST: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/request.json");
const QUOTE: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/quote.json");
const ACCEPT: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/accept.json");
const DELIVERY: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/delivery.json");
const VERDICT: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/verdict.json");
const ARTIFACT: &[u8] = include_bytes!("../../../fixtures/artifacts/reference.json");
const ORDER: &str = "8ed07cfeafd6b86b5c1a4501912ec6f1984442f1247bd79aca2a2f166433b51c";
const ENV: &str = "sim:fixture";

#[test]
fn sqlite_runtime_version_requires_the_patched_release() {
    for version in ["3.53.999", "3.9.99", "2.999.999"] {
        assert!(check_sqlite_version(version)
            .unwrap_err()
            .contains("3.54.0"));
    }
    for version in ["3.54.0", "3.54.1", "3.100.0", "4.0.0"] {
        assert_eq!(check_sqlite_version(version), Ok(()));
    }
    for invalid in [
        "",
        "3.54",
        "3..0",
        "+3.54.0",
        "3.54.0.1",
        "3.54.0-beta",
        "3.54.0 ",
        "3.054.0",
        "4294967296.54.0",
    ] {
        assert!(check_sqlite_version(invalid)
            .unwrap_err()
            .contains("unrecognized"));
    }
}

fn key(role: &str) -> SigningKey {
    let fixture: Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/golden/positive-signing-vector.json"
    ))
    .unwrap();
    SigningKey::from_bytes(&hex(fixture["keys"][role]["seed_hex"].as_str().unwrap()).unwrap())
}
fn public(key: &SigningKey) -> String {
    hex_string(key.verifying_key().as_bytes())
}
struct TestDir(PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn server() -> (TestDir, Shared) {
    let dir = TestDir(std::env::temp_dir().join(format!("settle463d-test-{}", random().unwrap())));
    std::fs::create_dir(&dir.0).unwrap();
    let issuer_seed_file = dir.0.join("issuer");
    std::fs::write(&issuer_seed_file, hex_string(key("coordinator").as_bytes())).unwrap();
    let config = Config {
        deployment: None,
        database: dir.0.join("service.db"),
        artifact_root: dir.0.join("artifacts"),
        environment: ENV.into(),
        clock: 1_700_000_000,
        buyer: public(&key("buyer")),
        opening_balance: 5_000_000,
        issuer_seed_file,
        admin_token: "ab".repeat(32),
    };
    (dir, open(config).await.unwrap())
}
struct Reply(StatusCode, Vec<u8>);
impl Reply {
    fn ok(&self) -> &[u8] {
        assert_eq!(
            self.0,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&self.1)
        );
        &self.1
    }
    fn json(&self) -> Value {
        serde_json::from_slice(self.ok()).unwrap()
    }
    fn error(self, status: u16, code: &str) {
        assert_eq!(
            self.0.as_u16(),
            status,
            "{}",
            String::from_utf8_lossy(&self.1)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&self.1).unwrap()["error"]["code"],
            code
        );
    }
}
async fn send(
    s: &Shared,
    method: &str,
    path: &str,
    bytes: &[u8],
    headers: &[(&str, &str)],
) -> Reply {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = http::dispatch(
        AxumState(s.clone()),
        request.body(HttpBody::from(bytes.to_vec())).unwrap(),
    )
    .await
    .unwrap_or_else(IntoResponse::into_response);
    Reply(
        response.status(),
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}
async fn publish(s: &Shared, route: &str, bytes: &[u8], idem: &str) -> Reply {
    send(s, "POST", route, bytes, &[("idempotency-key", idem)]).await
}
async fn authorization(
    s: &Shared,
    role: &str,
    identity: &SigningKey,
    method: &str,
    uri: &str,
) -> String {
    let key_hex = public(identity);
    let challenge = send(
        s,
        "POST",
        "/v1/auth/challenge",
        &serde_json::to_vec(&json!({"public_key":key_hex,"role":role})).unwrap(),
        &[],
    )
    .await
    .json();
    let (path, query_hash) = uri
        .split_once('?')
        .map(|(p, q)| (p, digest(q.as_bytes())))
        .unwrap_or((uri, "00".repeat(32)));
    let proof = preimage(b"0x463/http/0.1",&json!({
        "challenge_id":challenge["challenge_id"],"expires_at":challenge["expires_at"],"nonce":challenge["nonce"],
        "method":method,"path":path,"query_hash":query_hash,"public_key":key_hex,"role":role,
    })).unwrap();
    format!(
        "0x463 {}.{}",
        challenge["challenge_id"].as_str().unwrap(),
        hex_string(&identity.sign(&proof).to_bytes())
    )
}
async fn read(s: &Shared, role: &str, identity: &SigningKey, uri: &str) -> Reply {
    let proof = authorization(s, role, identity, "GET", uri).await;
    send(s, "GET", uri, &[], &[("authorization", &proof)]).await
}
async fn command(s: &Shared, role: &str, op: &str, cmd: &Value, idem: &str) -> Reply {
    let uri = format!("/v1/orders/{ORDER}/actions/{op}");
    let proof = authorization(s, role, &key(role), "POST", &uri).await;
    send(
        s,
        "POST",
        &uri,
        &serde_json::to_vec(cmd).unwrap(),
        &[("authorization", &proof), ("idempotency-key", idem)],
    )
    .await
}
async fn accepted(s: &Shared) {
    for (route, data) in [
        ("offers", OFFER),
        ("requests", REQUEST),
        ("quotes", QUOTE),
        ("acceptances", ACCEPT),
    ] {
        assert_eq!(
            publish(s, &format!("/v1/{route}"), data, "setup")
                .await
                .ok(),
            data
        );
    }
}
fn resign(original: &[u8], identity: &SigningKey, change: impl FnOnce(&mut Body)) -> SignedMessage {
    let mut msg = parse_signed(original, ENV).unwrap().unsigned().clone();
    msg.issuer = public(identity);
    change(&mut msg.payload);
    sign(msg, identity).unwrap()
}
async fn second_order(s: &Shared, buyer: &SigningKey) -> String {
    let buyer_id = public(buyer);
    let nonce = "33".repeat(32);
    let order = hex_string(&order_id(ENV, &buyer_id, &nonce).unwrap());
    let request = resign(REQUEST, buyer, |body| {
        let Body::Request(r) = body else { panic!() };
        r.buyer = buyer_id.clone();
        r.nonce = nonce;
        r.order_id = order.clone();
    });
    let quote = resign(QUOTE, &key("seller"), |body| {
        let Body::Quote(q) = body else { panic!() };
        q.request_id = hex_string(request.id());
        q.order_id = order.clone();
        q.buyer = buyer_id.clone();
        let Settlement::Sim {
            buyer_destination, ..
        } = &mut q.settlement
        else {
            panic!()
        };
        *buyer_destination = format!("sim:{buyer_id}");
    });
    let acceptance = resign(ACCEPT, buyer, |body| {
        let Body::Accept(a) = body else { panic!() };
        a.request_id = hex_string(request.id());
        a.order_id = order.clone();
        a.quote_id = hex_string(quote.id());
    });
    for m in [request, quote, acceptance] {
        publish(
            s,
            "/v1/messages",
            &m.to_wire().unwrap(),
            &hex_string(m.id()),
        )
        .await
        .ok();
    }
    order
}
async fn upload(s: &Shared, bytes: &[u8], idem: &str) -> Reply {
    let hash = digest(bytes);
    let len = bytes.len().to_string();
    let proof = preimage(
        b"0x463/artifact/0.1",
        &json!({"digest":hash,"length":len,"order_id":ORDER}),
    )
    .unwrap();
    let sig = hex_string(&key("seller").sign(&proof).to_bytes());
    send(
        s,
        "POST",
        &format!("/v1/orders/{ORDER}/artifacts"),
        bytes,
        &[
            ("content-type", "application/octet-stream"),
            ("x-artifact-digest", &hash),
            ("x-artifact-length", &len),
            ("x-artifact-signature", &sig),
            ("idempotency-key", idem),
        ],
    )
    .await
}

#[tokio::test]
async fn direct_quote_preserves_wire_bytes_and_private_read_consumes_challenge() {
    let (_dir, s) = server().await;
    publish(&s, "/v1/messages", OFFER, "offer").await.ok();
    let page = send(&s, "GET", "/v1/offers", &[], &[]).await;
    assert!(page.ok().windows(OFFER.len()).any(|bytes| bytes == OFFER));
    assert_eq!(page.json()["items"].as_array().unwrap().len(), 1);
    publish(&s, "/v1/messages", QUOTE, "premature")
        .await
        .error(404, "NOT_FOUND");
    publish(&s, "/v1/messages", REQUEST, "request").await.ok();
    publish(&s, "/v1/messages", QUOTE, "premature")
        .await
        .error(404, "NOT_FOUND");
    let original = publish(&s, "/v1/messages", QUOTE, "quote").await;
    assert_eq!(original.ok(), QUOTE);
    assert_eq!(
        publish(&s, "/v1/messages", QUOTE, "quote").await.ok(),
        QUOTE
    );
    let mut reformatted = QUOTE.to_vec();
    reformatted.push(b' ');
    publish(&s, "/v1/messages", &reformatted, "quote")
        .await
        .error(409, "IDEMPOTENCY_CONFLICT");
    let id = hex_string(parse_signed(QUOTE, ENV).unwrap().id());
    let path = format!("/v1/messages/{id}");
    assert_eq!(read(&s, "buyer", &key("buyer"), &path).await.ok(), QUOTE);
    publish(&s, "/v1/acceptances", ACCEPT, "accept").await.ok();
    publish(&s, "/v1/acceptances", ACCEPT, "new-key")
        .await
        .error(409, "ACCEPTANCE_EXISTS");
    let path = format!("/v1/orders/{ORDER}");
    let auth = authorization(&s, "buyer", &key("buyer"), "GET", &path).await;
    let headers = [("authorization", auth.as_str())];
    assert_eq!(
        send(&s, "GET", &path, &[], &headers).await.json()["state"],
        "Accepted"
    );
    send(&s, "GET", &path, &[], &headers)
        .await
        .error(401, "BAD_CHALLENGE");
    let events = read(
        &s,
        "buyer",
        &key("buyer"),
        &format!("{path}/events?limit=1"),
    )
    .await
    .json();
    let event = &events["items"][0];
    assert_eq!(events["next_cursor"], "1");
    assert_eq!(event["prev_digest"], "00".repeat(32));
    let original = serde_json::to_vec(&event["observation"]).unwrap();
    let observation = parse_signed(&original, ENV).unwrap();
    assert_eq!(observation.unsigned().issuer, public(&key("coordinator")));
    assert_eq!(
        event["event_digest"],
        event["observation"]["body"]["event_digest"]
    );
    read(&s, "buyer", &key("buyer"), &format!("{path}/receipt"))
        .await
        .error(404, "RECEIPT_NOT_READY");
}

#[tokio::test]
async fn relay_accepts_remote_issuer_quote_without_acquiring_settlement_authority() {
    let (_dir, relay) = server().await;
    let remote = SigningKey::from_bytes(&[42; 32]);
    let quote = resign(QUOTE, &key("seller"), |body| {
        let Body::Quote(q) = body else { panic!() };
        q.receipt_issuer = public(&remote);
    });
    let acceptance = resign(ACCEPT, &key("buyer"), |body| {
        let Body::Accept(a) = body else { panic!() };
        a.quote_id = hex_string(quote.id());
    });
    let quote_bytes = quote.to_wire().unwrap();
    for (name, bytes) in [
        ("offer", OFFER),
        ("request", REQUEST),
        ("quote", &quote_bytes),
    ] {
        assert_eq!(
            publish(&relay, "/v1/messages", bytes, name).await.ok(),
            bytes
        );
    }
    let path = format!("/v1/messages/{}", hex_string(quote.id()));
    assert_eq!(
        read(&relay, "buyer", &key("buyer"), &path).await.ok(),
        quote_bytes
    );
    publish(
        &relay,
        "/v1/messages",
        &acceptance.to_wire().unwrap(),
        "accept",
    )
    .await
    .ok();
    let order_path = format!("/v1/orders/{ORDER}");
    let order = read(&relay, "receipt_issuer", &remote, &order_path)
        .await
        .json();
    let review: Review = serde_json::from_value(order["review"].clone()).unwrap();
    assert_eq!(review.receipt_issuer, public(&remote));
    command(
        &relay,
        "buyer",
        "prepare",
        &json!({"action":"fund"}),
        "prepare",
    )
    .await
    .error(403, "ROLE_FORBIDDEN");
    let signing = review.signing_bytes().unwrap();
    let cmd = json!({
        "action":"fund",
        "buyer_signature":hex_string(&key("buyer").sign(&signing).to_bytes()),
        "seller_signature":hex_string(&key("seller").sign(&signing).to_bytes()),
    });
    command(&relay, "seller", "submit", &cmd, "submit")
        .await
        .error(403, "ROLE_FORBIDDEN");
    let state = read(&relay, "buyer", &key("buyer"), &order_path)
        .await
        .json();
    assert_eq!(state["state"], "Accepted");
    assert_eq!(state["buyer_balance"], "5000000");
    assert_eq!(state["held"], "0");
    read(
        &relay,
        "buyer",
        &key("buyer"),
        &format!("{order_path}/receipt"),
    )
    .await
    .error(404, "RECEIPT_NOT_READY");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM intents")
        .fetch_one(&relay.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn artifact_upload_is_bounded_immutable_and_authorized_per_order() {
    let (_dir, s) = server().await;
    accepted(&s).await;
    let a = upload(&s, ARTIFACT, "1").await.json();
    assert_eq!(a["length"], "52");
    assert_eq!(upload(&s, ARTIFACT, "2").await.json(), a);
    let b = upload(&s, b"changed bytes", "3").await.json();
    assert_ne!(b["content_id"], a["content_id"]);
    upload(&s, b"changed bytes", "1")
        .await
        .error(409, "IDEMPOTENCY_CONFLICT");
    let id = a["content_id"].as_str().unwrap();
    let path = format!("/v1/orders/{ORDER}/artifacts/{id}");
    for (role, fixture) in [
        ("buyer", "buyer"),
        ("seller", "seller"),
        ("verifier", "verifier"),
        ("receipt_issuer", "coordinator"),
    ] {
        assert_eq!(read(&s, role, &key(fixture), &path).await.ok(), ARTIFACT);
    }
    let stranger = SigningKey::from_bytes(&[99; 32]);
    let other = second_order(&s, &stranger).await;
    read(&s, "buyer", &stranger, &path)
        .await
        .error(403, "ARTIFACT_FORBIDDEN");
    read(
        &s,
        "buyer",
        &stranger,
        &format!("/v1/orders/{other}/artifacts/{id}"),
    )
    .await
    .error(403, "ARTIFACT_FORBIDDEN");
    send(
        &s,
        "POST",
        &format!("/v1/orders/{ORDER}/artifacts"),
        &[],
        &[("x-artifact-length", "1048577")],
    )
    .await
    .error(413, "OVERSIZED");
}

async fn funding(s: &Shared) -> (Value, String) {
    let review: Review = serde_json::from_value(
        command(s, "buyer", "prepare", &json!({"action":"fund"}), "prepare")
            .await
            .json(),
    )
    .unwrap();
    let bytes = review.signing_bytes().unwrap();
    (
        json!({
            "action":"fund", "buyer_signature":hex_string(&key("buyer").sign(&bytes).to_bytes()),
            "seller_signature":hex_string(&key("seller").sign(&bytes).to_bytes()),
        }),
        digest(&bytes),
    )
}

#[tokio::test]
async fn staged_signed_funding_recovers_and_retries_without_another_debit() {
    let (_dir, s) = server().await;
    accepted(&s).await;
    let (cmd, id) = funding(&s).await;
    let mut invalid = cmd.clone();
    invalid["buyer_signature"] = "00".repeat(64).into();
    command(&s, "buyer", "submit", &invalid, "forged")
        .await
        .error(401, "BAD_SIGNATURE");
    let submit_uri = format!("/v1/orders/{ORDER}/actions/submit");
    let mut tx = s.pool.begin().await.unwrap();
    assert_eq!(
        actions::enqueue(&s, &mut tx, ORDER, &cmd).await.unwrap(),
        id
    );
    sqlx::query("INSERT INTO idempotency(caller,operation,idem_key,path,request_hash,status,response,intent_id) VALUES(?,'submit','original',?,?,200,X'',?)")
        .bind(public(&key("buyer"))).bind(submit_uri).bind(digest(&serde_json::to_vec(&cmd).unwrap())).bind(&id)
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let config = s.config.clone();
    s.pool.close().await;
    let restored = open(config).await.unwrap();
    actions::recover(&restored).await.unwrap();
    let expected = json!({"intent_id":id,"outcome":"finalized_success"});
    for idem in ["original", "retry"] {
        assert_eq!(
            command(&restored, "buyer", "submit", &cmd, idem)
                .await
                .json(),
            expected
        );
    }
    let mut extra = cmd.clone();
    extra["extra"] = true.into();
    command(&restored, "buyer", "submit", &extra, "new")
        .await
        .error(400, "UNKNOWN_FIELD");
    let state = read(
        &restored,
        "buyer",
        &key("buyer"),
        &format!("/v1/orders/{ORDER}"),
    )
    .await
    .json();
    assert_eq!(state["buyer_balance"], "3000000");
    assert_eq!(state["held"], "2000000");
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM intents),(SELECT count(*) FROM outbox),(SELECT count(*) FROM audit_events WHERE event_type='finalized')").fetch_one(&restored.pool).await.unwrap();
    assert_eq!(counts, (1, 0, 1));
}

#[tokio::test]
async fn missing_finalized_history_keeps_the_intent_retryable_with_one_gap_observation() {
    let (_dir, s) = server().await;
    accepted(&s).await;
    let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let id = "12".repeat(32);
    sqlx::query(
        "INSERT INTO intents(intent_id,order_id,action,state,signed_bytes,local_signature)
         VALUES(?,?,'release','submitted',x'0102','original')",
    )
    .bind(&id)
    .bind(ORDER)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO outbox(intent_id,available_at) VALUES(?,'0')")
        .bind(&id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let before_retry = http::now().unwrap();
    for _ in 0..2 {
        let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let row = sqlx::query("SELECT * FROM intents WHERE intent_id=?")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        chain::defer_for_history(&s, &mut tx, &row, "missing preceding finalized transaction")
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let mut db = s.pool.acquire().await.unwrap();
    let stats: (i64, i64, i64, String, Vec<u8>) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM receipts),
            (SELECT COUNT(*) FROM audit_events WHERE event_type='reconciliation_gap'),
            outbox.attempts,outbox.last_error,intents.signed_bytes
         FROM outbox JOIN intents USING(intent_id)
         WHERE intent_id=? AND intents.state='unknown' AND CAST(outbox.available_at AS INTEGER)>=?",
    )
    .bind(&id)
    .bind(before_retry + 2)
    .fetch_one(&mut *db)
    .await
    .unwrap();
    assert_eq!(stats, (0, 1, 2, "RECONCILIATION_GAP".into(), vec![1, 2]));
    let status: Value =
        serde_json::from_slice(&actions::status(&mut db, ORDER, &id).await.unwrap()).unwrap();
    assert_eq!(status["outcome"], "unknown");
}

#[tokio::test]
async fn submitted_validity_metadata_must_match_the_prepared_message_and_order() {
    let (_dir, s) = server().await;
    accepted(&s).await;
    let mut db = s.pool.acquire().await.unwrap();
    let message = b"prepared message";
    sqlx::query("INSERT INTO solana_preparations VALUES(?,'fund',?,'12','180')")
        .bind(ORDER)
        .bind(digest(message))
        .execute(&mut *db)
        .await
        .unwrap();
    for (order, action, bytes, slot, height) in [
        (ORDER, "fund", message.as_slice(), 13, 180),
        (ORDER, "fund", message.as_slice(), 12, u64::MAX),
        (ORDER, "fund", b"other".as_slice(), 12, 180),
        (ORDER, "refund", message.as_slice(), 12, 180),
        ("other", "fund", message.as_slice(), 12, 180),
    ] {
        assert!(
            chain::consume_preparation(&mut db, order, action, bytes, slot, height)
                .await
                .is_err()
        );
    }
    chain::consume_preparation(&mut db, ORDER, "fund", message, 12, 180)
        .await
        .unwrap();
    assert!(
        chain::consume_preparation(&mut db, ORDER, "fund", message, 12, 180)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn sticky_verdict_pays_once_and_exports_one_signed_receipt() {
    let (_dir, s) = server().await;
    accepted(&s).await;
    let (cmd, id) = funding(&s).await;
    assert_eq!(
        command(&s, "buyer", "submit", &cmd, "fund").await.json()["intent_id"],
        id
    );
    let artifact = upload(&s, ARTIFACT, "artifact").await.json();
    let delivery = resign(DELIVERY, &key("seller"), |body| {
        let Body::Delivery(d) = body else { panic!() };
        d.content_id = artifact["content_id"].as_str().unwrap().into();
    });
    publish(
        &s,
        "/v1/deliveries",
        &delivery.to_wire().unwrap(),
        "delivery",
    )
    .await
    .ok();
    let mut commit = json!({"action":"commit_delivery","delivery_id":hex_string(delivery.id())});
    let review: ActionReview = serde_json::from_value(
        command(&s, "seller", "prepare", &commit, "commit")
            .await
            .json(),
    )
    .unwrap();
    commit["signature"] = hex_string(
        &key("seller")
            .sign(&review.signing_bytes().unwrap())
            .to_bytes(),
    )
    .into();
    assert_eq!(
        command(&s, "seller", "submit", &commit, "commit")
            .await
            .json()["intent_id"],
        review.intent_id().unwrap()
    );
    let verdict = resign(VERDICT, &key("verifier"), |body| {
        let Body::Verdict(v) = body else { panic!() };
        v.delivery_id = hex_string(delivery.id());
    });
    let wire = verdict.to_wire().unwrap();
    assert_eq!(publish(&s, "/v1/verdicts", &wire, "pass").await.ok(), wire);
    let fail = resign(&wire, &key("verifier"), |body| {
        let Body::Verdict(v) = body else { panic!() };
        v.decision = "FAIL".into();
        v.reason_codes = vec!["SCHEMA_MISMATCH".into()];
    });
    assert_eq!(
        publish(&s, "/v1/verdicts", &fail.to_wire().unwrap(), "changed")
            .await
            .ok(),
        wire
    );
    let mut release = json!({"action":"release","verdict_id":hex_string(verdict.id())});
    let review: ActionReview = serde_json::from_value(
        command(&s, "verifier", "prepare", &release, "release")
            .await
            .json(),
    )
    .unwrap();
    release["signature"] = hex_string(
        &key("verifier")
            .sign(&review.signing_bytes().unwrap())
            .to_bytes(),
    )
    .into();
    assert_eq!(
        command(&s, "verifier", "submit", &release, "release")
            .await
            .json()["intent_id"],
        review.intent_id().unwrap()
    );
    command(&s, "verifier", "submit", &release, "new")
        .await
        .ok();
    let path = format!("/v1/orders/{ORDER}/receipt");
    let bundle = read(&s, "buyer", &key("buyer"), &path).await;
    assert_eq!(
        bundle.ok(),
        read(&s, "verifier", &key("verifier"), &path).await.ok()
    );
    let trust = settle463_core::receipt::ReceiptTrust {
        trusted_issuer: public(&key("coordinator")),
        trusted_order: ORDER.into(),
        environment: ENV.into(),
    };
    settle463_core::receipt::verify_sim_bundle(bundle.ok(), &trust).unwrap();
    let state = read(&s, "buyer", &key("buyer"), &format!("/v1/orders/{ORDER}"))
        .await
        .json();
    assert_eq!(state["seller_balance"], "2000000");
    assert_eq!(state["held"], "0");
}
