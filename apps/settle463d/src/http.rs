use super::{actions, db, *};
use axum::{
    body::{to_bytes, Body as HttpBody},
    extract::{Query, Request, State as AxumState},
    http::{header, HeaderMap, Method, Uri},
};
use ed25519_dalek::{Signature, VerifyingKey};
use settle463_core::{
    ident::{decimal, timestamp},
    parse_signed,
    wire::Body,
};
use sqlx::Row;
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;

const MAX_BYTES: usize = 1_048_576;
struct Caller {
    key: String,
    role: String,
}

fn header_value<'a>(headers: &'a HeaderMap, key: &str) -> Result<&'a str> {
    headers
        .get(key)
        .and_then(|h| h.to_str().ok())
        .ok_or(ApiError::bad("BAD_ENCODING"))
}
fn now() -> Result<i64> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApiError::store())?
            .as_secs(),
    )
    .map_err(|_| ApiError::store())
}
fn role_ok(role: &str) -> bool {
    matches!(role, "buyer" | "seller" | "verifier" | "receipt_issuer")
}
async fn issue_challenge(s: &State, bytes: &[u8]) -> Result<Vec<u8>> {
    let v = canonical::parse(bytes)?;
    let key = field(&v, "public_key")?;
    hex::<32>(key)?;
    let role = field(&v, "role")?;
    if !role_ok(role) {
        return Err(ApiError::bad("BAD_ENCODING"));
    }
    let id = random()?;
    let nonce = random()?;
    let expiry = now()? + 60;
    let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM challenges WHERE expires_at<=?")
        .bind(now()?)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO challenges(challenge_id,nonce,public_key,role,expires_at) VALUES(?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&nonce)
    .bind(key)
    .bind(role)
    .bind(expiry)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    json_bytes(json!({"challenge_id":id,"nonce":nonce,"expires_at":expiry.to_string()}))
}

async fn authenticate(
    s: &State,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Caller> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(ApiError::auth)?;
    let (id, sig) = auth
        .strip_prefix("0x463 ")
        .and_then(|h| h.split_once('.'))
        .ok_or_else(ApiError::auth)?;
    let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row=sqlx::query("SELECT nonce,public_key,role,expires_at FROM challenges WHERE challenge_id=? AND used=0 AND expires_at>?")
        .bind(id).bind(now()?).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::auth)?;
    let key: String = row.get("public_key");
    let role: String = row.get("role");
    let bytes = preimage(
        b"0x463/http/0.1",
        &json!({"challenge_id":id,"expires_at":row.get::<i64,_>("expires_at").to_string(),
        "method":method.as_str(),"nonce":row.get::<String,_>("nonce"),"path":uri.path(),"public_key":key,
        "query_hash":uri.query().filter(|q|!q.is_empty()).map(|q|digest(q.as_bytes())).unwrap_or_else(||"00".repeat(32)),"role":role}),
    )?;
    let public = VerifyingKey::from_bytes(&hex(&key)?).map_err(|_| ApiError::auth())?;
    public
        .verify_strict(
            &bytes,
            &Signature::from_bytes(&hex(sig).map_err(|_| ApiError::auth())?),
        )
        .map_err(|_| ApiError::auth())?;
    let used = sqlx::query(
        "UPDATE challenges SET used=1 WHERE challenge_id=? AND used=0 AND expires_at>?",
    )
    .bind(id)
    .bind(now()?)
    .execute(&mut *tx)
    .await?;
    if used.rows_affected() != 1 {
        return Err(ApiError::auth());
    }
    tx.commit().await?;
    Ok(Caller { key, role })
}

async fn require_member(s: &State, order: &str, c: &Caller, artifact: bool) -> Result<()> {
    if !db::member(&mut *s.pool.acquire().await?, order, &c.key, &c.role).await? {
        return Err(ApiError::denied(if artifact {
            "ARTIFACT_FORBIDDEN"
        } else {
            "ROLE_FORBIDDEN"
        }));
    }
    Ok(())
}
fn response(bytes: Vec<u8>, mime: &'static str) -> Response {
    ([(header::CONTENT_TYPE, mime)], HttpBody::from(bytes)).into_response()
}
pub(crate) async fn dispatch(
    AxumState(s): AxumState<Shared>,
    request: Request,
) -> Result<Response> {
    let (parts, body) = request.into_parts();
    let uri = parts.uri;
    let headers = parts.headers;
    let method = parts.method;
    let path: Vec<&str> = uri.path().trim_start_matches('/').split('/').collect();
    if method == Method::GET {
        let (bytes, mime) = get(&s, &uri, &headers, &path).await?;
        return Ok(response(bytes, mime));
    }
    if method != Method::POST {
        return Err(ApiError::missing());
    }
    if path.len() == 4
        && path[1] == "orders"
        && path[3] == "artifacts"
        && [
            headers.get("x-artifact-length"),
            headers.get(header::CONTENT_LENGTH),
        ]
        .into_iter()
        .flatten()
        .any(|h| {
            h.to_str()
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|n| n > MAX_BYTES as u64)
        })
    {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "OVERSIZED"));
    }
    let bytes = to_bytes(body, MAX_BYTES)
        .await
        .map_err(|_| ApiError(StatusCode::PAYLOAD_TOO_LARGE, "OVERSIZED"))?;
    if path == ["v1", "auth", "challenge"] {
        return Ok(response(
            issue_challenge(&s, &bytes).await?,
            "application/json",
        ));
    }
    let key = header_value(&headers, "idempotency-key")?;
    if key.is_empty() || key.len() > 256 {
        return Err(ApiError::bad("BAD_ENCODING"));
    }
    let result = post(&s, &uri, &path, &headers, key, &bytes).await?;
    Ok(response(result, "application/json"))
}

fn query(uri: &Uri) -> Result<HashMap<String, String>> {
    Ok(Query::<HashMap<String, String>>::try_from_uri(uri)
        .map_err(|_| ApiError::bad("BAD_ENCODING"))?
        .0)
}
fn page(q: &HashMap<String, String>) -> Result<(i64, i64)> {
    let cursor = q.get("cursor").map(String::as_str).unwrap_or("0");
    let limit = q.get("limit").map(String::as_str).unwrap_or("100");
    let cur: i64 = decimal(cursor)?
        .try_into()
        .map_err(|_| ApiError::bad("BAD_ENCODING"))?;
    let count = decimal(limit)?;
    if !(1..=100).contains(&count) {
        return Err(ApiError::bad("BAD_ENCODING"));
    }
    Ok((cur, count as i64))
}
fn paged(mut rows: Vec<(i64, Vec<u8>)>, n: i64) -> Result<Vec<u8>> {
    let more = rows.len() > n as usize;
    if more {
        rows.pop();
    }
    let cursor = if more {
        rows.last().map(|r| r.0.to_string())
    } else {
        None
    };
    let list = rows
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.1))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    json_bytes(json!({"items":list,"next_cursor":cursor}))
}
async fn get(
    s: &State,
    uri: &Uri,
    h: &HeaderMap,
    path: &[&str],
) -> Result<(Vec<u8>, &'static str)> {
    let json = "application/json";
    match path{
        ["v1","health"]=>Ok((json_bytes(json!({"status":"ok","version":"0.1"}))?,json)),
        ["v1","sim","clock"]=>{
            let clock=db::clock(&mut *s.pool.acquire().await?).await?;
            Ok((json_bytes(json!({"clock":clock.to_string(),"environment":s.config.environment}))?,json))
        }
        ["v1","offers"]=>{
            let q=query(uri)?;let env=q.get("environment").map(String::as_str).unwrap_or(&s.config.environment);
            let service=q.get("service_id");
            let (cur,n)=page(&q)?;
            let rows=sqlx::query_as("SELECT rowid,signed_bytes FROM signed_messages WHERE kind='service_offer' AND environment=? AND (? IS NULL OR service_id=?) AND rowid>? ORDER BY rowid LIMIT ?")
                .bind(env).bind(service).bind(service).bind(cur).bind(n+1).fetch_all(&s.pool).await?;
            Ok((paged(rows,n)?,json))
        }
        ["v1","offers",id]=>Ok((sqlx::query_scalar("SELECT signed_bytes FROM signed_messages WHERE kind='service_offer' AND message_id=?").bind(id).fetch_one(&s.pool).await?,json)),
        ["v1","messages",id]=>{
            let caller=authenticate(s,&Method::GET,uri,h).await?;
            let q=query(uri)?;let order=q.get("order_id").ok_or_else(ApiError::auth)?;
            let mut conn=s.pool.acquire().await?;
            let bytes:Vec<u8>=sqlx::query_scalar("SELECT signed_bytes FROM signed_messages WHERE message_id=? AND order_id=?").bind(id).bind(order).fetch_one(&mut *conn).await?;
            let message=parse_signed(&bytes,&s.config.environment)?;
            let allowed=match &message.unsigned().payload{
                Body::Request(r)=>match caller.role.as_str(){"buyer"=>caller.key==r.buyer,"seller"=>caller.key==r.seller,_=>false},
                Body::Quote(q)=>match caller.role.as_str(){"buyer"=>caller.key==q.buyer,"seller"=>caller.key==q.seller,"verifier"=>caller.key==q.verifier,"receipt_issuer"=>caller.key==q.receipt_issuer,_=>false},
                _=>false
            };
            if !allowed && !db::member(&mut conn,order,&caller.key,&caller.role).await?{return Err(ApiError::denied("ROLE_FORBIDDEN"));}
            Ok((bytes,json))
        }
        ["v1","orders",order,rest @ ..]=>{
            let c=authenticate(s,&Method::GET,uri,h).await?;
            let artifact=rest.first()==Some(&"artifacts");require_member(s,order,&c,artifact).await?;
            let mut conn=s.pool.acquire().await?;
            let bytes=match *rest {
                []=>db::order_projection(s,&mut conn,order).await?,
                ["receipt"]=>sqlx::query_scalar("SELECT bundle FROM receipts WHERE order_id=?").bind(order).fetch_optional(&mut *conn).await?
                    .ok_or(ApiError(StatusCode::NOT_FOUND,"RECEIPT_NOT_READY"))?,
                ["intents",id]=>actions::status(&mut conn,order,id).await?,
                ["events"]=>{
                    let q=query(uri)?;let (cur,n)=page(&q)?;
                    let rows=sqlx::query("SELECT e.seq,e.event_type,e.payload_hash,e.prev_digest,e.event_digest,m.signed_bytes FROM audit_events e JOIN signed_messages m ON m.message_id=e.observation_id WHERE e.order_id=? AND e.seq>? ORDER BY e.seq LIMIT ?")
                    .bind(order).bind(cur).bind(n+1).fetch_all(&mut *conn).await?;
                    let mut result=Vec::new();
                    for row in rows{
                        let seq:i64=row.get("seq");let obs:Value=serde_json::from_slice(row.get("signed_bytes"))?;
                        result.push((seq,json_bytes(json!({"order_id":order,"seq":seq.to_string(),"event_type":row.get::<String,_>("event_type"),"prev_digest":row.get::<String,_>("prev_digest"),
                            "payload_hash":row.get::<String,_>("payload_hash"),"event_digest":row.get::<String,_>("event_digest"),"observation":obs}))?));
                    }
                    paged(result,n)?
                }
                ["artifacts",id]=>{
                    hex::<32>(id)?;
                    let row:Option<(String,String)>=sqlx::query_as("SELECT storage_key,digest FROM artifacts WHERE order_id=? AND content_id=?").bind(order).bind(id).fetch_optional(&mut *conn).await?;
                    let (key,expected)=row.ok_or(ApiError::denied("ARTIFACT_FORBIDDEN"))?;
                    hex::<32>(&key)?;let file=s.config.artifact_root.join(&key[..2]).join(&key);
                    let data=tokio::fs::read(file).await.map_err(|_|ApiError::store())?;
                    if digest(&data)!=expected{return Err(ApiError::store());}
                    return Ok((data,"application/octet-stream"))
                }
                _=>return Err(ApiError::missing())
            };
            Ok((bytes,json))
        }
        _=>Err(ApiError::missing())
    }
}

struct Idem<'a> {
    caller: &'a str,
    operation: &'a str,
    key: &'a str,
    path: &'a str,
    hash: String,
}
async fn cached(db: &mut Conn, i: &Idem<'_>) -> Result<Option<Vec<u8>>> {
    let row=sqlx::query("SELECT path,request_hash,response,intent_id FROM idempotency WHERE caller=? AND operation=? AND idem_key=?")
      .bind(i.caller).bind(i.operation).bind(i.key).fetch_optional(db).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.get::<String, _>("path") != i.path || row.get::<String, _>("request_hash") != i.hash {
        return Err(ApiError::conflict("IDEMPOTENCY_CONFLICT"));
    }
    if row.get::<Option<String>, _>("intent_id").is_some()
        && row.get::<Vec<u8>, _>("response").is_empty()
    {
        return Ok(None);
    }
    Ok(Some(row.get("response")))
}
async fn save(db: &mut Conn, i: &Idem<'_>, response: &[u8], intent: Option<&str>) -> Result<()> {
    sqlx::query("INSERT INTO idempotency(caller,operation,idem_key,path,request_hash,status,response,intent_id) VALUES(?,?,?,?,?,200,?,?) ON CONFLICT(caller,operation,idem_key) DO UPDATE SET response=excluded.response")
        .bind(i.caller).bind(i.operation).bind(i.key).bind(i.path).bind(&i.hash).bind(response).bind(intent).execute(db).await?;
    Ok(())
}

async fn upload(
    s: &State,
    db: &mut Conn,
    order: &str,
    h: &HeaderMap,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    if header_value(h, "content-type")? != "application/octet-stream" {
        return Err(ApiError::bad("BAD_ENCODING"));
    }
    let length = decimal(header_value(h, "x-artifact-length")?)?;
    let hash = digest(bytes);
    if length != bytes.len() as u64 || header_value(h, "x-artifact-digest")? != hash {
        return Err(ApiError::bad("BAD_ENCODING"));
    }
    let agreement = db::agreement(db, &s.config.environment, order).await?;
    let (_, q) = agreement.terms(&s.config.environment)?;
    if db::clock(db).await? >= timestamp(&q.deadlines.deliver_by)? {
        return Err(ApiError::conflict("STATE_CONFLICT"));
    }
    let stored: Option<String> =
        sqlx::query_scalar("SELECT content_id FROM artifacts WHERE order_id=? AND digest=?")
            .bind(order)
            .bind(&hash)
            .fetch_optional(&mut *db)
            .await?;
    let id = if let Some(id) = stored {
        id
    } else {
        let id = random()?;
        let dir = s.config.artifact_root.join(&id[..2]);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|_| ApiError::store())?;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&id))
            .await
            .map_err(|_| ApiError::store())?;
        file.write_all(bytes).await.map_err(|_| ApiError::store())?;
        file.sync_all().await.map_err(|_| ApiError::store())?;
        tokio::fs::File::open(&dir)
            .await
            .map_err(|_| ApiError::store())?
            .sync_all()
            .await
            .map_err(|_| ApiError::store())?;
        sqlx::query("INSERT INTO artifacts VALUES(?,?,?,?,?)")
            .bind(&id)
            .bind(order)
            .bind(&hash)
            .bind(length.to_string())
            .bind(&id)
            .execute(&mut *db)
            .await?;
        db::audit(
            s,
            db,
            order,
            "artifact_stored",
            &json!({"content_id":id,"digest":hash,"length":length.to_string()}),
            vec![id.clone()],
        )
        .await?;
        id
    };
    json_bytes(json!({"content_id":id,"length":length.to_string(),"digest":hash}))
}

async fn post(
    s: &State,
    uri: &Uri,
    path: &[&str],
    h: &HeaderMap,
    key: &str,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    match path {
        ["v1", route @ ("offers" | "requests" | "quotes" | "acceptances" | "deliveries" | "verdicts"
        | "messages")] => {
            let msg = parse_signed(bytes, &s.config.environment)?;
            let valid = matches!(
                (*route, db::kind(&msg)),
                ("offers", "service_offer")
                    | ("requests", "request")
                    | ("quotes", "quote")
                    | ("acceptances", "accept")
                    | ("deliveries", "delivery")
                    | ("verdicts", "verdict")
                    | ("messages", _)
            );
            if !valid {
                return Err(ApiError::bad("UNSUPPORTED_KIND"));
            }
            let i = Idem {
                caller: &msg.unsigned().issuer,
                operation: route,
                key,
                path: uri.path(),
                hash: digest(bytes),
            };
            let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await?;
            if let Some(saved) = cached(&mut tx, &i).await? {
                return Ok(saved);
            }
            let result = db::publish(s, &mut tx, &msg, bytes).await?;
            save(&mut tx, &i, &result, None).await?;
            tx.commit().await?;
            Ok(result)
        }
        ["v1", "sim", "clock"] => {
            if h.get("x-sim-admin").and_then(|h| h.to_str().ok())
                != Some(s.config.admin_token.as_str())
            {
                return Err(ApiError::denied("ROLE_FORBIDDEN"));
            }
            let i = Idem {
                caller: "sim-admin",
                operation: "clock",
                key,
                path: uri.path(),
                hash: digest(bytes),
            };
            let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await?;
            if let Some(saved) = cached(&mut tx, &i).await? {
                return Ok(saved);
            }
            let target = timestamp(field(&canonical::parse(bytes)?, "clock")?)?;
            if target < db::clock(&mut tx).await? {
                return Err(ApiError::conflict("STATE_CONFLICT"));
            }
            sqlx::query("UPDATE deployment SET clock=?")
                .bind(target.to_string())
                .execute(&mut *tx)
                .await?;
            let result = json_bytes(json!({"clock":target.to_string()}))?;
            save(&mut tx, &i, &result, None).await?;
            tx.commit().await?;
            Ok(result)
        }
        ["v1", "orders", order, rest @ ..] => {
            hex::<32>(order)?;
            let artifact = *rest == ["artifacts"];
            let c = if artifact && h.get("x-artifact-signature").is_some() {
                let mut conn = s.pool.acquire().await?;
                let seller: String =
                    sqlx::query_scalar("SELECT seller FROM orders WHERE order_id=?")
                        .bind(order)
                        .fetch_one(&mut *conn)
                        .await?;
                let msg = preimage(
                    b"0x463/artifact/0.1",
                    &json!({"digest":header_value(h,"x-artifact-digest")?,"length":header_value(h,"x-artifact-length")?,"order_id":order}),
                )?;
                let pk = VerifyingKey::from_bytes(&hex(&seller)?).map_err(|_| ApiError::auth())?;
                pk.verify_strict(
                    &msg,
                    &Signature::from_bytes(&hex(header_value(h, "x-artifact-signature")?)?),
                )
                .map_err(|_| ApiError::auth())?;
                Caller {
                    key: seller,
                    role: "seller".into(),
                }
            } else {
                authenticate(s, &Method::POST, uri, h).await?
            };
            require_member(s, order, &c, artifact).await?;
            let (op, act) = match *rest {
                ["artifacts"] if c.role == "seller" => ("artifacts", Value::Null),
                ["artifacts"] => return Err(ApiError::denied("ARTIFACT_FORBIDDEN")),
                ["actions", op @ ("prepare" | "submit")] => {
                    let v = canonical::parse(bytes)?;
                    let allowed = match field(&v, "action")? {
                        "fund" => c.role == "buyer" || c.role == "seller",
                        "commit_delivery" => c.role == "seller",
                        "release" => c.role == "verifier",
                        "refund" => true,
                        _ => return Err(ApiError::bad("BAD_ENCODING")),
                    };
                    if !allowed {
                        return Err(ApiError::denied("ROLE_FORBIDDEN"));
                    }
                    (op, v)
                }
                _ => return Err(ApiError::missing()),
            };
            let i = Idem {
                caller: &c.key,
                operation: op,
                key,
                path: uri.path(),
                hash: digest(bytes),
            };
            let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await?;
            if let Some(saved) = cached(&mut tx, &i).await? {
                return Ok(saved);
            }
            if op == "submit" {
                let intent = actions::enqueue(s, &mut tx, order, &act).await?;
                save(&mut tx, &i, &[], Some(&intent)).await?;
                tx.commit().await?;
                #[cfg(debug_assertions)]
                if std::env::var("SETTLE463_TEST_STOP").as_deref() == Ok("before_broadcast") {
                    std::process::exit(86);
                }
                actions::process(s, &intent).await?;
                let mut tx = s.pool.begin_with("BEGIN IMMEDIATE").await?;
                let response = actions::status(&mut tx, order, &intent).await?;
                save(&mut tx, &i, &response, Some(&intent)).await?;
                tx.commit().await?;
                return Ok(response);
            }
            let result = if artifact {
                upload(s, &mut tx, order, h, bytes).await?
            } else {
                actions::prepare(s, &mut tx, order, &act).await?
            };
            save(&mut tx, &i, &result, None).await?;
            tx.commit().await?;
            Ok(result)
        }
        _ => Err(ApiError::missing()),
    }
}
