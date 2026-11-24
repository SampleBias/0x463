# HTTP API v1

## Service

`apps/settle463d` is an Axum process. SQLite is the only database. Bind loopback HTTP with no TLS. Any non-loopback listener requires TLS via `rustls`. The simulator backend does not open a socket to a chain. The Solana backend uses a configured RPC.

Public discovery returns signed offers and nothing else. Quotes, acceptances, artifacts, destinations, and receipts are private.

## Authentication

Mutations that submit a protocol message use the message signature as authorization. The route's required issuer role must match the [issuer table](wire.md#envelope). The server verifies the signature, the environment, and the links before it commits.

Private reads and action routes that do not embed a new protocol signature use a one-time challenge.

1. `POST /v1/auth/challenge` with JSON `{"public_key":"<hex32>","role":"buyer|seller|verifier|receipt_issuer"}`.
2. The server stores a 32-byte nonce, a 60-second expiry, and a single-use flag.
3. The response is `{"challenge_id":"<hex32>","expires_at":"<decimal>","nonce":"<hex32>"}`.
4. The client signs the ASCII prefix `0x463/http/0.1`, one `0x00` byte, and the JCS bytes of:

```json
{
  "challenge_id": "<hex32>",
  "expires_at": "<decimal>",
  "method": "GET",
  "nonce": "<hex32>",
  "path": "/v1/orders/<order_id>",
  "public_key": "<hex32>",
  "query_hash": "<hex sha256 of the raw query string, or 64 zero hex chars when the query is empty>",
  "role": "buyer"
}
```

5. The header is `Authorization: 0x463 <challenge_id>.<signature hex>`.

This HTTP preimage is not a protocol kind. The server marks the challenge used before it returns the private body. A reused challenge is `401`. Role membership is evaluated against the order: the public key must be that order's buyer, seller, verifier, or receipt issuer. Receipt export is restricted to those same parties.

## Idempotency

Every POST except `POST /v1/auth/challenge` requires the header `Idempotency-Key`. The key is scoped to the authenticated caller public key plus the route pattern. The server stores the SHA-256 of the raw request body.

| Situation | Result |
| --- | --- |
| Same caller, route, key, and body | The original status and body. |
| Same caller, route, and key, different body | `409` and error code `IDEMPOTENCY_CONFLICT`. |
| New key, same economic request | The uniqueness rules reject a second accept, fund, delivery, release, or refund. |

## Routes

| Method and path | Auth | Behavior |
| --- | --- | --- |
| `GET /v1/health` | None | `{"status":"ok","version":"0.1"}` |
| `GET /v1/offers` | None | Cursor page of signed offers. Query: `environment`, optional `service_id`, optional `cursor`, optional `limit` up to 100. |
| `GET /v1/offers/{offer_id}` | None | One signed offer. |
| `POST /v1/offers` | Offer signature | Store the offer. Trusted-publisher filtering is the client's job. The server stores any structurally valid offer. |
| `POST /v1/auth/challenge` | None | Issue a challenge. |
| `POST /v1/requests` | Request signature | Store the request. |
| `POST /v1/quotes` | Quote signature | Store the quote after linkage checks. |
| `POST /v1/acceptances` | Accept signature | Insert the acceptance. Uniqueness failure is `409` `ACCEPTANCE_EXISTS`. |
| `POST /v1/orders/{order_id}/artifacts` | Seller challenge or signed delivery intent | Upload. See [artifact store](#artifact-store). |
| `GET /v1/orders/{order_id}/artifacts/{content_id}` | Challenge | Return the raw bytes. |
| `POST /v1/deliveries` | Delivery signature | Store the delivery message. |
| `POST /v1/verdicts` | Verdict signature | Apply the sticky PASS/FAIL rule. |
| `POST /v1/orders/{order_id}/actions/prepare` | Challenge of a party allowed to run that action | Return the unsigned review object. |
| `POST /v1/orders/{order_id}/actions/submit` | Challenge, plus the signed transaction or the simulator command | Persist intent, enqueue outbox, return submission state. |
| `GET /v1/orders/{order_id}` | Challenge | Order projection. |
| `GET /v1/orders/{order_id}/events` | Challenge | Cursor page of audit events. |
| `GET /v1/orders/{order_id}/receipt` | Challenge | Receipt bundle once the outcome is terminal and finalized. `404` `RECEIPT_NOT_READY` before that. |
| `POST /v1/messages` | The embedded message signature | Direct-exchange ingest of any supported kind. Same validation as the dedicated route. |

Cursor responses use `{"items":[...],"next_cursor":"<opaque or null>"}`. Clients poll events by cursor. The cursor is the decimal sequence as an opaque string. Clients treat it as opaque.

Prepare and submit bodies name the action: `fund`, `commit_delivery`, `release`, or `refund`.

Error body:

```json
{"error":{"code":"ACCEPTANCE_EXISTS","message":"one acceptance already exists for this request"}}
```

| HTTP | Codes |
| --- | --- |
| 400 | `MALFORMED_JSON`, `DUPLICATE_FIELD`, `UNKNOWN_FIELD`, `BAD_ENCODING`, `UNSUPPORTED_KIND`, `UNSUPPORTED_VERSION` |
| 401 | `BAD_CHALLENGE`, `BAD_SIGNATURE` |
| 403 | `ROLE_FORBIDDEN`, `ARTIFACT_FORBIDDEN` |
| 404 | `NOT_FOUND`, `RECEIPT_NOT_READY` |
| 409 | `IDEMPOTENCY_CONFLICT`, `ACCEPTANCE_EXISTS`, `NONCE_REUSED`, `ALREADY_COMPLETE`, `STATE_CONFLICT` |
| 422 | `POLICY_REJECTED`, `LINKAGE_FAILED`, `DEADLINE_ORDER`, `ENVIRONMENT_MISMATCH`, `AMOUNT_REJECTED`, `SCHEMA_REJECTED` |
| 503 | `STORE_UNAVAILABLE`, `LEDGER_UNAVAILABLE` |

## Artifact store

The seller uploads after acceptance and before `deliver_by`. The request body is the raw bytes, with a maximum of 1048576 bytes enforced by the server limit before the body is stored. Headers:

- `Content-Type: application/octet-stream`
- `X-Artifact-Length: <decimal>`
- `X-Artifact-Digest: <hex sha256>`
- `Authorization` challenge for the seller, or a seller-signed delivery-intent header whose preimage is `0x463/artifact/0.1`, `0x00`, then JCS of `{"digest","length","order_id"}`.

The store computes SHA-256 itself and rejects a mismatched header. The content id is a fresh 32-byte hex value. The file path is `{root}/{content_id[0:2]}/{content_id}` and both segments are validated hex. User input is never joined into the path.

Re-upload of the same order and the same digest returns the existing content id. A different digest gets a new content id. Bytes for a content id never change.

`GET` returns the raw bytes to the order's buyer, seller, verifier, or receipt issuer. Any other caller receives `403` `ARTIFACT_FORBIDDEN`. Possession of the digest or the content id is not authorization.

The reference relay's outage is reported as `503` `STORE_UNAVAILABLE`. The verifier maps that to `INDETERMINATE` / `UNAVAILABLE`.

## Direct exchange

SDK trait `MessageTransport`:

- `publish(message) -> Result<()>`
- `fetch(message_id) -> Result<SignedMessage>`

Implementations: `HttpCoordinator` and `DirectPeer` with a configured base URL. Both carry the same signed bytes. The coordinator is not required for two peers who can reach each other. Refund and inspect use the settlement backend and the signed bundle, so they still run when the coordinator is down.
