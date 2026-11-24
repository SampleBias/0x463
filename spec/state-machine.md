# Escrow state machine

## Clock

Authoritative time is the simulator's injected clock or the Solana `Clock` sysvar `unix_timestamp` at instruction execution. A wall clock may schedule retries. A wall clock cannot accept or reject a transfer.

Deadlines are absolute Unix seconds. They satisfy `fund_by < deliver_by < resolve_by`. `fund_by` is less than or equal to the request expiry and less than or equal to the quote expiry.

At the exact deadline second, the expiring action is invalid and the timeout path is valid.

| Boundary | At `now == deadline` |
| --- | --- |
| `fund_by` | `fund` is invalid. No financial receipt is created for an order that never funded. |
| `deliver_by` | `commit_delivery` is invalid. `refund` from `Funded` is valid. |
| `resolve_by` | `release` is invalid. `refund` from `Delivered` is valid. |

One tick before a deadline, the still-open action is valid and the timeout path is invalid. One tick after, the timeout path stays valid and the expired action stays invalid.

## State machine

| Current | Command | Authority and preconditions | Result |
| --- | --- | --- | --- |
| Unfunded | `fund` | Buyer and seller. Terms valid. `now < fund_by`. Exact principal. | Funded. Principal held. |
| Funded | `commit_delivery` | Seller. `now < deliver_by`. First commitment. | Delivered. Delivery id stored. |
| Delivered | `release` | Designated verifier. PASS report. `now < resolve_by`. Stored delivery id present. | Paid. Exact principal to the seller destination. Verdict id stored. |
| Funded | `refund` | Any caller. `now >= deliver_by`. | Refunded. Exact principal to the buyer destination. |
| Delivered | `refund` | Any caller. `now >= resolve_by`. | Refunded. Exact principal to the buyer destination. |

`Paid` and `Refunded` are terminal. A replay returns the known result or the stable already-completed error and moves zero principal. A terminal order cannot be reopened.

There is no early refund on FAIL. A FAIL report is evidence. The refund deadline stays in force. `INDETERMINATE` is per attempt and retryable. The first definitive PASS or FAIL for a committed delivery is persisted. Later verdict calls return that same signed decision.

A previously signed PASS submitted at or after `resolve_by` cannot pay.

Verification of bytes happens off-chain. On-chain release enforcement is the designated verifier's signature over the release transaction. The verifier service signs that transaction only for the exact PASS report.

## Money invariant

For every order, after any successful or attempted operation:

```text
funded principal = principal still held + principal paid + principal refunded
```

Use checked integer arithmetic. The maximum seller payout is the funded amount. Refund only to the committed buyer destination. SOL transaction fees, account rent, and unsolicited tokens that land in the vault are outside principal. No operator instruction can withdraw or redirect principal.

Unsolicited vault deposits stay in the vault. Terminalization transfers exactly the funded principal. A nonempty vault does not block terminalization.

A payment submission, an observed unfinalized transfer, and a verifier PASS are three different states. None of them is finalized payment. A receipt is emitted only after finalized terminal success and a matching escrow observation.

Reusing a buyer nonce on one deployment creates one `order_id`. At most one escrow deposit exists for that `order_id`.
