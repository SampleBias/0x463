# API, persistence, receipts, and CLI

Phase A publishes this contract as `spec/api-v1.md`, `spec/persistence.md`, and `spec/receipts.md`. Phase C implements it. The CLI, the SDK, and `settle463d` share `settle463-core`. Private keys never travel to the API.

## Service

`apps/settle463d` is an Axum process. SQLite is the only database. Bind loopback HTTP with no TLS. Any non-loopback listener requires TLS via `rustls`. The simulator backend does not open a socket to a chain. The Solana backend uses a configured RPC.

Public discovery returns signed offers and nothing else. Quotes, acceptances, artifacts, destinations, and receipts are private.

## Authentication

Mutations that submit a protocol message use the message signature as authorization. The route's required issuer role must match the kind table in `01-wire-contract.md`. The server verifies the signature, the environment, and the links before it commits.

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
| `POST /v1/orders/{order_id}/artifacts` | Seller signature over the raw-byte digest header | Upload. See artifact section. |
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

## Audit hash

For event sequence `n` of an order, the digest preimage is ASCII `0x463/event/0.1`, one `0x00`, then JCS of:

```json
{
  "event_type": "<name>",
  "order_id": "<hex>",
  "payload_hash": "<hex sha256 of canonical payload>",
  "prev_digest": "<hex or 64 zero chars when seq is 1>",
  "seq": "<decimal>"
}
```

The coordinator then signs an `observation` over that digest. Event types: `offer_published`, `request_stored`, `quote_stored`, `accepted`, `artifact_stored`, `intent_recorded`, `submitted`, `finalized`, `reconciliation_gap`, `receipt_issued`.

## Tables

SQLite migrations live in `apps/settle463d/migrations/`. Amounts, lengths, and timestamps are `TEXT` decimal. Compare-and-set uses `version`.

| Table | Keys and rules |
| --- | --- |
| `signed_messages` | Primary key `message_id`. Columns for kind, environment, issuer, `signed_bytes`, canonical bytes, signature. |
| `acceptances` | Primary key `(environment, buyer, nonce)`. Unique `request_id`. Unique `order_id`. |
| `orders` | Primary key `order_id`. Projection state, ids, roles, principal text, `held`, `paid`, `refunded`, destinations, deadlines, `delivery_id`, `verdict_id`, `version`. |
| `artifacts` | Primary key `content_id`. Unique `(order_id, digest)`. Internal storage key. |
| `idempotency` | Primary key `(caller, operation, idem_key)`. Request hash and stored response. |
| `audit_events` | Primary key `(order_id, seq)`. `prev_digest`, `event_digest`, observation message id. |
| `intents` | Primary key `intent_id`. Order, action, state, signed transaction bytes, local signature, attempt counter. At most one non-terminal intent per `(order_id, action)`. |
| `outbox` | Primary key integer. `intent_id`, `available_at`, attempts, `locked_until`, last error. Inserted in the same transaction as the intent. |
| `receipts` | Primary key `order_id`. Unique receipt message id. Bundle JSON. |
| `challenges` | Primary key `challenge_id`. Expiry and used flag. |

Principal columns for one order always satisfy the money invariant. Updates that would break it roll back.

Intent states: `prepared`, `submitted`, `finalized_success`, `finalized_failure`, `expired`, `unknown`.

## Settlement trait

Defined in `settle463-core` so the SDK does not depend on Solana:

```text
prepare_fund(agreement) -> Review
submit_fund(review, signatures) -> Submission
prepare_commit_delivery(...)
submit_commit_delivery(...)
prepare_release(...)
submit_release(...)
prepare_refund(...)
submit_refund(...)
inspect(order_id) -> OrderView
reconcile(intent_id) -> ChainOutcome
```

`Review` contains every instruction, every account, the amount, the mint or asset id, the destinations, the deadlines, and the message ids the signer must compare. `ChainOutcome` is `Pending`, `FinalizedSuccess`, `FinalizedFailure`, `Expired`, or `Unknown`.

The simulator implements the trait with an atomic in-memory or SQLite ledger, per-asset balances, a settable clock, and unique orders. A simulated receipt sets `ledger_kind` to `sim`. Pending submissions exist as a separate state so recovery tests can observe a submitted intent that is not yet finalized.

The Solana adapter implements the same trait. Business rules for amounts, deadlines, and terminality are the core rules. The adapter adds transaction encoding, RPC, and finality.

## Recovery procedure

1. Insert the intent and the outbox row before any broadcast.
2. Build the transaction, run the signer policy, store the signed bytes and the signature, then broadcast.
3. On restart, load unresolved intents, including `prepared`, `submitted`, and `unknown`.
4. If signed bytes exist, fetch that signature at finalized commitment and fetch the escrow account.
5. Reconcile each action against its transaction and the finalized escrow. Persist a proven fund or delivery once. If the escrow is terminal, suppress further economic submissions, identify the actual release or refund signature, and verify its transfer and the preceding actions. Insert the unique receipt for that terminal outcome. A terminal account with missing transaction history leaves a reconciliation gap and the receipt pending. [Action evidence](04-solana-contract.md#adapter-reconciliation)
6. If execution is uncertain, retain the signed bytes, mark the attempt `unknown`, and continue reconciliation. Absence from a non-archival RPC is insufficient to authorize rebuilding.
7. Once expiry and non-execution are established and the escrow still permits the action, rebuild, revalidate, re-sign, store the new signature, and submit.
8. A finalized failed transaction leaves the escrow in its previous state. The attempt is recorded. A later eligible action may be prepared as a new intent for a different action, or as a retry of the same action when the failure was a closed token account or a transient error.
9. An unfinalized observation that disappears returns the intent to `unknown`. Retry once eligibility and non-execution are established. Receipt issuance follows step 5.

The test harness creates the crash by persisting the intent and the signed transaction, broadcasting, discarding the RPC response, and constructing a new process from the same database. No production endpoint exists to skip verification.

## Receipt bundle

```json
{
  "acceptance": {},
  "bundle_version": "0.1",
  "delivery": {},
  "environment": "sim:<id>",
  "ledger": {},
  "observations": [],
  "order_id": "<hex>",
  "quote": {},
  "receipt": {},
  "request": {},
  "verdict": {}
}
```

`delivery` and `verdict` are omitted when the outcome does not include them. The bundle excludes artifact bytes, private keys, challenge nonces, and temporary URLs.

Simulator `ledger`:

```json
{
  "asset_id": "test-six-decimal",
  "clock": "1700010800",
  "decimals": "6",
  "deployment_id": "<id>",
  "destination": "sim:<hex>",
  "finality": "simulated-final",
  "ledger_kind": "sim",
  "principal": "2000000",
  "terminal_locator": "sim:<order_id>:<seq>"
}
```

Solana `ledger` includes `ledger_kind` `solana`, genesis base58, program id, config, order PDA, mint, atomic amount, beneficiary, vault, funding signature, optional delivery signature, terminal signature, terminal instruction name (`release` or `refund`), finalized slot as a decimal string, and `evidence_digest`. The digest is SHA-256 of the JCS ledger object with `evidence_digest` removed.

## Verification modes

Offline checks:

- Each included message verifies under the strict signature rules.
- The receipt issuer matches the trust input.
- The order id matches the trust input and the derived id.
- Linkage, policy hash, amount, asset, and outcome rules hold.
- A timeout before delivery has no delivery and no verdict.
- A timeout after delivery has a delivery and no PASS verdict.
- The result label is `evidence_signed`.

Online checks add the caller's RPC and pinned deployment:

- `getGenesisHash` equals the trusted genesis.
- The trusted program id and config match the environment.
- The funding transaction is finalized, succeeded, and names the agreement ids and the exact principal.
- If a delivery exists, the commit transaction is finalized and stores that delivery id.
- The terminal transaction is finalized, succeeded, and moves the exact principal from the vault to the receipt destination.
- Apply the same per-action transaction and escrow checks as the reconciler. [Action evidence](04-solana-contract.md#adapter-reconciliation)
- The result label is `ledger_verified`.

| Result | Exit code |
| --- | --- |
| `evidence_signed` or `ledger_verified` | 0 |
| Usage or missing trust input | 1 |
| `invalid_evidence` | 2 |
| `untrusted` | 3 |
| `pending` | 4 |
| `unverifiable` | 5 |

`unverifiable` is the archival gap: the RPC cannot return the transaction and cannot prove it failed. A transaction that is present and contradictory is `invalid_evidence`. Trust fields inside the bundle are ignored as anchors.

## CLI

Binary package `settle463` in `apps/settle463`.

```text
settle463 demo --backend sim --scenario pass
settle463 demo --backend sim --scenario bad-artifact
settle463 demo --backend sim --scenario no-delivery
settle463 demo --backend sim --scenario verifier-offline
settle463 order inspect <order-id> --trust-file <path>
settle463 receipt verify <bundle.json> --offline --trust-file <path>
settle463 receipt verify <bundle.json> --rpc <approved-endpoint> --trust-file <path>
```

`--json` prints one JSON object and keeps logs on stderr. `--trust-file` supplies `trusted_issuer`, `trusted_order`, `environment`, and, for Solana, `genesis` and `program_id`. Flags of the same names override the file. Missing trust input exits 1.

The demo starts `settle463d` on an ephemeral loopback port with a temporary database, drives it through the HTTP SDK, and stops it. The demo does not call crate-private test helpers.

Plain stdout lines, in this order:

```text
order_id=<hex>
backend=sim
outcome=<outcome>
asset=<asset id>
principal=2000000
delta_buyer=<signed decimal>
delta_seller=<signed decimal>
delta_escrow=0
receipt=<path>
```

| Scenario | Outcome | Buyer delta | Seller delta |
| --- | --- | --- | --- |
| `pass` | `paid` | `-2000000` | `2000000` |
| `bad-artifact` | `refunded_timeout_after_delivery` | `0` | `0` |
| `no-delivery` | `refunded_timeout_before_delivery` | `0` | `0` |
| `verifier-offline` | `refunded_timeout_after_delivery` | `0` | `0` |

Deltas are measured from the pre-fund balances to the terminal balances. The buyer starts with `5000000` atomic units so a rejected second fund is rejected by the protocol rather than by an empty balance. `bad-artifact` uploads the one-byte mutation `units` `"4"`, receives FAIL, advances the simulator clock to `resolve_by`, and refunds. `verifier-offline` commits the good artifact, records no verdict, advances to `resolve_by`, and refunds.

`order inspect` prints the state, the three principal buckets, the deadlines, and the message ids.

Local-validator and devnet demos use the same stdout contract with `backend=local-validator` or `backend=devnet`. Phase D and phase E document the extra flags for RPC, mint, and key paths.

## SDK surface

Public types in `settle463-sdk`: `BuyerClient`, `SellerClient`, `VerifierClient`, `BuyerPolicy`, `SellerPolicy`, `MessageTransport`, and `SettlementBackend`.

Minimum methods:

- Buyer: `discover`, `create_request`, `accept`, `read_artifact`, `inspect`, `export_receipt`, `refund_if_eligible`.
- Seller: `publish_offer`, `quote`, `upload_artifact`, `commit_delivery`, `sign_fund`.
- Verifier: `fetch_artifact`, `judge`, `release_if_pass`.

`accept` and `quote` run policy checks and return the error `POLICY_REJECTED` before any signature function is called. Examples in `crates/settle463-sdk/examples/` show each actor against the simulator. Phase F links those examples from the README.
