# Simulator settlement

Each live simulator uses its own deployment ID in `sim:<deployment-id>`. The ID has 1 to 64 lowercase ASCII letters, digits or hyphens and begins and ends with a letter or digit. The deterministic signing fixtures use `sim:fixture`.

The asset is `{"decimals":"6","id":"test-six-decimal","kind":"sim"}`. A party account is `sim:<agent hex>`, and the escrow hold account is `sim-escrow:<order_id>`. The conformance buyer starts with `5000000` atomic units and the seller starts with zero. The worked order funds `2000000` atomic units.

All transfers, state changes and ledger events for an action commit atomically. Apply the deadline comparisons against the injected ledger clock at execution. An unsuccessful or replayed action moves zero principal. Inspect returns the current state, deadlines, message IDs and the held, paid and refunded buckets. [State transitions and conservation](state-machine.md)

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

On terminal success the simulator locator is `sim:<order_id>:<decimal event seq>`. Its finality label is `simulated-final`. The receipt names the exact destination and funded principal. [Receipt bundle](receipts.md#receipt-bundle)

## Simulator funding authorization

The buyer stores one signed acceptance for each tuple of environment, buyer key and nonce. Both parties retrieve that acceptance before signing the funding command. Each independently checks the request, signed quote, acceptance and the supplied review. The buyer rechecks its spending policy. The seller rechecks its minimum amount, verifier and deployment policy.

The simulator funding review is a closed JSON object. Its fields are `environment`, `order_id`, `request_id`, `quote_id`, `acceptance_id`, `policy_hash`, `buyer`, `seller`, `verifier`, `receipt_issuer`, `principal`, `asset`, `buyer_destination`, `seller_destination`, `escrow_account` and `deadlines`. The value of `principal` is the accepted quote's decimal amount. The buyer and seller destinations are copied from the quote. The escrow account is `sim-escrow:<order_id>`. The instruction debits the buyer destination and credits the escrow account by exactly the principal.

Each party signs the ASCII domain `0x463/sim-fund/0.1`, one zero byte and the canonical JSON bytes of that review. The intent ID is lowercase hex SHA-256 of those same bytes. The simulator reconstructs the review from the three verified envelopes, verifies both Ed25519 signatures, and applies the check `ledger_time < fund_by` at execution. Manual finalization records a pending intent and defers the transfer until that execution succeeds. The buyer's delivery window is `deliver_by - fund_by` and its resolution window is `resolve_by - deliver_by`.
