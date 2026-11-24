# Phase B — Signed negotiation and the local economy

Status target: two agents fund one simulator escrow through the public SDK, and the funding-authorization negatives fail closed.

Depends on: phase A gate.

Blocks: phase C.

## Goal

Implement identities, buyer and seller policy, signed offers, quotes, single-use acceptance, and simulator funding. Run the flow through `BuyerClient` and `SellerClient`. The simulator is the ledger. No network and no real currency.

## Entry

- Phase A evidence exists and its tests still pass.
- Wire types from `settle463-core` are the only message types. Do not create a parallel struct for quotes.

## Exit gate

1. T-NEG-01 through T-NEG-08 and T-BIND-01, T-BIND-03, T-BIND-04, T-BIND-05, T-BIND-08, T-BIND-09, T-BIND-10, and T-BIND-11 pass.
2. The passing run uses two distinct keypairs and calls public SDK methods. The test does not construct a funded ledger row by writing a private field.
3. A quote with no acceptance moves no balance (T-NEG-08).
4. `cargo test --locked -p settle463-core -p settle463-sim -p settle463-sdk` passes.
5. `build/phase-b-evidence.md` lists commands, the order id of T-NEG-01, and the balance snapshot after fund.

## Out of scope

Artifact bytes, verifier judgments, receipts, HTTP, CLI demos, Solana transactions. Deadline boundary tests that need delivery or release wait for phase C, except `fund_by`, which this phase includes as part of funding authorization.

## WP-B1 — Keys and policy

In `settle463-sdk`:

- `Identity` wraps an Ed25519 signing key and exposes the hex public key and, for Solana later, the base58 form from the same bytes.
- `BuyerPolicy` and `SellerPolicy` match `01-wire-contract.md`.
- `BuyerClient::create_request` and `BuyerClient::accept` call the policy check and return `POLICY_REJECTED` without calling `sign` when the check fails. A test can wrap the signing key in a spy or compare message counts by inspecting the returned error path: no `SignedMessage` is returned on rejection.
- Generate the request nonce with the operating system's cryptographic RNG. Tests may inject a nonce so T-NEG-06 is deterministic.

The demo and tests mark fixture keys with the seed domain `0x463/fixture-seed/0.1` only when they intentionally reproduce the golden identities. Live simulator tests use fresh keys so a fixture key cannot be mistaken for a generated party.

## WP-B2 — Simulator ledger

Implement `SettlementBackend` for the simulator in `settle463-sim`.

State:

- Per-asset balances keyed by `(account, asset_id)`.
- Orders keyed by `order_id`.
- Injected clock as `i64` seconds. `set_clock` is public on the simulator type and is unavailable on the Solana adapter.
- Append-only command log so a test can show that a rejected command added no transfer.

`fund` is atomic: debit the buyer, credit the escrow hold, and set state `Funded` in one critical section, or change nothing. Use checked `u64` arithmetic. Opening buyer balance in tests is `5000000` of `test-six-decimal`. Seller starts at `0`.

Reject:

- Principal other than the accepted quote amount.
- `now >= fund_by`.
- A second fund for the same order id.
- Mismatched environment, mint or asset id, or destination.
- Zero principal.

On success, `held = principal`, `paid = 0`, `refunded = 0`.

Account ids are `sim:<agent hex>`. The escrow hold account is `sim-escrow:<order_id>`.

Provide `inspect` that returns state and the three buckets. Provide a pending-submission flag on the simulator intent so phase C recovery tests can leave an intent submitted and unfinalized. In this phase, `submit_fund` may finalize immediately unless the test enables manual finalization.

A simulated receipt is not emitted yet. Do not call a receipt final. Phase C owns receipts. Phase B's inspect result is enough to prove a deposit.

## WP-B3 — Negotiation state

Implement in the SDK, storing messages in the simulator process or in an in-memory `MessageStore` behind `MessageTransport`. HTTP comes in phase C. The trait from `02-api-persistence.md` exists now with an in-memory implementation so phase C can add `HttpCoordinator` without changing callers.

Flow for the happy path:

1. Seller `publish_offer` signs a `service_offer` for `sample-json-report` and `sim:<fresh-deployment>`.
2. Buyer filters offers by the configured trusted publisher set.
3. Buyer `create_request` derives `order_id`, checks policy, and signs the request with `max_principal` `2500000`.
4. Seller `quote` signs the quote for `2000000`, the reference digest and length, the reference acceptance policy, deadlines with the same offsets as `00-locked-decisions.md`, and `fees.protocol_atomic` `"0"`.
5. Buyer `accept` rechecks policy, signs `accept`, and stores the acceptance under `(environment, buyer, nonce)` before `sign_fund`.
6. Both parties review a `Review` object from `prepare_fund`. The review includes amount, asset, destinations, deadlines, and ids.
7. Both sign the simulator fund command. `submit_fund` moves balances.

Uniqueness:

- The acceptance store's insert is transactional. Two accepts for one `request_id` result in one row.
- The same nonce derives the same `order_id` and the second fund fails.
- A new quote is a new message id. The buyer may accept only one of them.

The seller signs the fund command only when the review equals the seller's quote. The seller has no API that signs `release`.

## WP-B4 — Tests through the SDK

Put tests in `crates/settle463-sdk/tests/negotiation.rs` or `tests/conformance/negotiation.rs`. They construct `BuyerClient` and `SellerClient` with distinct keys and an in-memory transport plus the simulator backend.

Cover every T-NEG and T-BIND row assigned to phase B. For T-BIND-11, sign a request in `sim:fixture` and submit it to a client whose pinned environment is a different `sim:` id.

For the `fund_by` boundary, set the clock to `fund_by - 1`, fund a dedicated order successfully, and show that another order at `now == fund_by` does not fund and does not produce a financial receipt. That is the phase B portion of T-TIME-01.

Assert the money invariant after every successful fund and every rejected fund.

## Commands

```text
cargo fmt --all --check
cargo clippy -p settle463-core -p settle463-sim -p settle463-sdk --all-targets -- -D warnings
cargo test --locked -p settle463-core -p settle463-sim -p settle463-sdk
```

## Evidence

`build/phase-b-evidence.md` includes:

- Commands and exit codes.
- T-NEG-01 order id, environment, buyer and seller public keys (hex), amount, and the three principal buckets.
- A sentence that the test entry point is `BuyerClient` / `SellerClient`.
- Confirmation that clippy was denied warnings on these crates.

## Stop conditions

- The only way to fund is a test-only function that skips signatures. Stop and route the happy path through the SDK.
- Policy rejection happens after `sign`. Move the check earlier.
- Balances use floating point or SOL lamports mixed into the token principal. Replace that accounting.

## Handoff

Phase C adds the verifier, the blob store, receipts, the HTTP service, and the CLI on top of this SDK. It should not create a second order state machine. Timeout refunds and delivery commands extend the same `SettlementBackend`.
