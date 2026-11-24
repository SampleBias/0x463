# Phase B funding evidence

Local run on 2026-11-24, based on Phase A Git revision `963abb07d2b26f9f3c59da672812f45873009a95` with the Phase B working-tree changes. The compiler and Cargo were pinned to 1.89.0. Tests ran offline with `CARGO_HOME=/tmp/settle463-domain-cargo` and `CARGO_NET_OFFLINE=true`.

## One funded order

The test entered through the public `SellerClient` and `BuyerClient` methods, then submitted both signatures through `SettlementBackend::submit_fund`. It generated fresh keys, a random deployment and a random request nonce. Reruns produce different identifiers.

| Field | Observed value |
| --- | --- |
| Environment | `sim:910a644c0f13c13c2a358f7e9f2f7865` |
| Order ID | `f7e674c6e05b13e82a83ab7e05e2f6a66a0e14b3a8548e740cb6c5a1f5eb0e3e` |
| Buyer key | `cba9de5424d3cc9a39b58ef8f1ee53eef352a387ef764412fd6a31d145336a3c` |
| Seller key | `e4fd5c9476d0bd40d4a6995640026bc9df7c19a6b019149263e6227ef37ce991` |
| Asset | `test-six-decimal` |
| Buyer opening / closing balance | 5000000 / 3000000 |
| Seller closing balance | 0 |
| Principal / held / paid / refunded | 2000000 / 2000000 / 0 / 0 |
| State | Funded |

## Commands

| Command | Exit | Result |
| --- | ---: | --- |
| `cargo fmt --all --check` | 0 | Workspace formatting passed. |
| `cargo clippy -p settle463-core -p settle463-sim -p settle463-sdk --all-targets -- -D warnings` | 0 | Clippy checked the three crates with warnings denied, including examples and tests. |
| `cargo test --locked -p settle463-core -p settle463-sim -p settle463-sdk` | 0 | Eight existing core tests and six negotiation tests passed. |
| `cargo test --locked -p settle463-sdk --test negotiation negotiated_fund_and_replay -- --exact --nocapture` | 0 | Produced the T-NEG-01 snapshot above. |
| `cargo run --quiet --offline --locked -p settle463-sdk --example negotiate` | 0 | Reported `Funded held=2000000 paid=0 refunded=0 buyer_balance=3000000 seller_balance=0`. |
| `python3 fixtures/tools/independent_vector.py` | 0 | Phase A fixture identities, signatures and byte comparisons passed. |

## Conformance

| Cases | Observable result |
| --- | --- |
| T-NEG-01, T-NEG-08 | A quote leaves balances untouched. A linked acceptance and two funding signatures produce exactly one transfer. |
| T-NEG-02, Phase B portion of T-TIME-01 | One order funds at `fund_by - 1`. At `fund_by`, a second order cannot fund and a pending order expires with zero transfer. |
| T-NEG-03, T-BIND-08, T-BIND-11 | A different deployment's signed request and the signed `sim:fixture` request are rejected. |
| T-NEG-04, T-BIND-01 | The buyer rejects a correctly signed quote over its cap. Changing a signed or reviewed amount cannot debit the buyer. |
| T-NEG-05, T-NEG-06, T-BIND-10 | Concurrent accepts select one quote. Distinct requests sharing a nonce cannot reserve a second acceptance or produce a second deposit. |
| T-NEG-07, T-BIND-03, T-BIND-04, T-BIND-05, T-BIND-09 | Policy, deadline and participant changes are rejected. The seller cannot be selected as verifier. |
| T-BIND-02 | A modified envelope signature and missing or exchanged funding signatures are rejected. |
| Manual finalization | Three separately accepted pending orders compete for a balance sufficient for two. Exactly two finalize successfully. Reconciliation and repeated finalization preserve the balances. |

The [SDK example](../crates/settle463-sdk/examples/negotiate.rs) reproduces the public flow. The [simulator contract](../spec/settlement-sim.md#simulator-funding-authorization) defines the bytes that both parties sign. Receipt generation and terminal transfers belong to Phase C.
