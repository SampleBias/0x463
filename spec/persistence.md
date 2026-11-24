# Persistence

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

## Recovery procedure

1. Insert the intent and the outbox row before any broadcast.
2. Build the transaction, run the signer policy, store the signed bytes and the signature, then broadcast.
3. On restart, load unresolved intents, including `prepared`, `submitted`, and `unknown`.
4. If signed bytes exist, fetch that signature at finalized commitment and fetch the escrow account.
5. Reconcile each action against its transaction and the finalized escrow. Persist a proven fund or delivery once. If the escrow is terminal, suppress further economic submissions, identify the actual release or refund signature, and verify its transfer and the preceding actions. Insert the unique receipt for that terminal outcome. A terminal account with missing transaction history leaves a reconciliation gap and the receipt pending. [Action evidence](settlement-solana.md#adapter-reconciliation)
6. If execution is uncertain, retain the signed bytes, mark the attempt `unknown`, and continue reconciliation. Absence from a non-archival RPC is insufficient to authorize rebuilding.
7. Once expiry and non-execution are established and the escrow still permits the action, rebuild, revalidate, re-sign, store the new signature, and submit.
8. A finalized failed transaction leaves the escrow in its previous state. The attempt is recorded. A later eligible action may be prepared as a new intent for a different action, or as a retry of the same action when the failure was a closed token account or a transient error.
9. An unfinalized observation that disappears returns the intent to `unknown`. Retry once eligibility and non-execution are established. Receipt issuance follows step 5.
