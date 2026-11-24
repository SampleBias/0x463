# Conformance matrix

Each row names the phase that first runs it. Later phases that repeat the case on another backend say so in the phase file. A row is done only when the command column has been executed and the result is in that phase's evidence file.

Backend words: `sim`, `validator`, `devnet`.

## Golden and wire

| Id | Phase | Case | Command shape | Pass condition |
| --- | --- | --- | --- | --- |
| T-GOLD-01 | A | Independent vector | `cargo test -p settle463-core --test golden` | Message id, signature, order id, and policy hash match `fixtures/golden/positive-signing-vector.json`. |
| T-GOLD-02 | A | One-byte canonical change | same test | Changed preimage yields a new message id and strict verification fails. |
| T-GOLD-03 | A | UTF-16 key order and JCS string encoding | `cargo test -p settle463-core --test jcs` | Emoji key sorts before U+E000. Both spellings in the line-separator test produce exactly `7b22e280a8223a22e280a9227d`. |
| T-GOLD-04 | A | Duplicate key | `cargo test -p settle463-core --test json_reject` | `{"a":"1","a":"2"}` is rejected before map collapse. |
| T-GOLD-05 | A | OpenSSL or Python recomputation | `python3 fixtures/tools/independent_vector.py` | The script reprints the golden hex values and exits 0. |
| T-WIRE-01 | A | Negative encodings | `cargo test -p settle463-core --test negative_wire` | Uppercase hex, unknown field, bad version, number token, null, and bad environment are rejected. |

## Negotiation and funding authorization

| Id | Phase | Case | Pass condition |
| --- | --- | --- | --- |
| T-NEG-01 | B | Happy offer, request, quote, accept, fund | SDK run on `sim`. State `Funded`. Principal held equals `2000000`. |
| T-NEG-02 | B | Expired quote | Accept or fund fails. No deposit. |
| T-NEG-03 | B | Wrong environment | Message rejected. No deposit. |
| T-NEG-04 | B | Amount above buyer maximum | SDK returns `POLICY_REJECTED` before a signature is produced. No deposit. |
| T-NEG-05 | B | Two concurrent accepts of different quotes for one request | One acceptance row. The other returns `ACCEPTANCE_EXISTS`. |
| T-NEG-06 | B | Reused buyer nonce | One order id. At most one fund. |
| T-NEG-07 | B | Seller named as verifier | Quote and fund rejected. |
| T-NEG-08 | B | Quote alone | No acceptance row and no balance change. |

## Artifact, verdict, and the four demos

| Id | Phase | Case | Pass condition |
| --- | --- | --- | --- |
| T-CLI-01 | C | `demo --backend sim --scenario pass` | Exit 0. Outcome `paid`. Deltas `-2000000` / `2000000` / `0`. Receipt path printed. |
| T-CLI-02 | C | `bad-artifact` | Outcome `refunded_timeout_after_delivery`. Verdict FAIL. Digests differ. No release. |
| T-CLI-03 | C | `no-delivery` | Outcome `refunded_timeout_before_delivery`. Bundle has no delivery and no verdict. |
| T-CLI-04 | C | `verifier-offline` | Delivery exists. No verdict. Refund at `resolve_by`. |
| T-CLI-05 | C | Fresh-process offline verify of each demo bundle | New process, exit 0, result `evidence_signed`. |
| T-CLI-06 | C | `order inspect` | Prints the same order id and terminal state. |
| T-ART-01 | C | Buyer reads the pass artifact | SDK returns the 52 reference bytes. |
| T-ART-02 | C | Schema-invalid bytes whose digest is the committed digest | Separate order. Reasons include `SCHEMA_MISMATCH` and exclude `DIGEST_MISMATCH`. No release. |
| T-ART-03 | C | Malformed, duplicate key, oversized | No PASS and no release. |
| T-ART-04 | C | Read of another order's content id | `403` `ARTIFACT_FORBIDDEN`. |
| T-ART-05 | C | Idempotent re-upload | Same content id. Different bytes get a new id and do not change the old bytes. |

## Time boundaries

Run on `sim` in phase C and again on `validator` in phase D for fund, delivery, release, and both refunds.

| Id | Boundary | One tick before | Exactly on the boundary | One tick after |
| --- | --- | --- | --- | --- |
| T-TIME-01 | `fund_by` | Fund succeeds | Fund fails. No receipt. | Fund fails. |
| T-TIME-02 | `deliver_by` from `Funded` | Delivery succeeds. Refund fails. | Delivery fails. Refund succeeds. | Refund succeeds. Delivery fails. |
| T-TIME-03 | `resolve_by` from `Delivered` with PASS | Release succeeds. | Release fails. Refund succeeds. | Release fails. Refund succeeds. |
| T-TIME-04 | Silent verifier | At `resolve_by - 1` refund fails. | Refund succeeds. | Refund succeeds. |
| T-TIME-05 | PASS signed early, submitted at `resolve_by` | — | Release fails. Refund remains available. | — |

## Binding and replay

| Id | Phase | Mutation | Pass condition |
| --- | --- | --- | --- |
| T-BIND-01 | B | Cap or amount changed after the quote | Rejected. |
| T-BIND-02 | A/B | Signature bit flipped | Rejected. |
| T-BIND-03 | B | Policy object changed without a new hash | Rejected. |
| T-BIND-04 | B | Deadline changed | Rejected. |
| T-BIND-05 | B | Participant key changed | Rejected. |
| T-BIND-06 | D | Destination changed | Signer policy and program reject it. |
| T-BIND-07 | D | Mint changed | Program returns `Mint`. |
| T-BIND-08 | B | Environment swapped for another deployment | Rejected. |
| T-BIND-09 | B | Seller key placed in the verifier field | Rejected. |
| T-BIND-10 | B | Nonce reused | One deposit. |
| T-BIND-11 | B | Signed message from `sim:fixture` submitted to a live demo environment | Rejected. |

## Idempotency and conservation

| Id | Phase | Case | Pass condition |
| --- | --- | --- | --- |
| T-IDEM-01 | C | Duplicate fund | One deposit. Second call is `AlreadyComplete` or the known funded result. |
| T-IDEM-02 | C | Duplicate delivery | One delivery id. |
| T-IDEM-03 | C | Duplicate release | One payout. Seller balance increases once. |
| T-IDEM-04 | C | Concurrent release and refund | One terminal state. Principal conserved. |
| T-IDEM-05 | C | Repeated refund | Zero additional transfer. |
| T-IDEM-06 | C | Same HTTP idempotency key and body | Original response. |
| T-IDEM-07 | C | Same HTTP key, different body | `409` `IDEMPOTENCY_CONFLICT`. |
| T-IDEM-08 | C | Fresh HTTP keys on a retry of fund | Still one deposit. |
| T-IDEM-09 | C | Terminal order fund attempt | State unchanged. |

Phase D repeats T-IDEM-01, T-IDEM-03, T-IDEM-04, and T-IDEM-05 against the validator using token balances.

## Recovery

| Id | Phase | Case | Pass condition |
| --- | --- | --- | --- |
| T-REC-01 | C on sim, D on validator | Crash after the intent write and before broadcast | Restart submits once and reaches the correct terminal state. |
| T-REC-02 | D | Broadcast succeeded, RPC response discarded, process restarted | Recover a fund once with zero receipts while funded. Recover a terminal transfer once with one receipt. A later read of a terminal escrow still verifies the original funding and delivery transactions. |
| T-REC-03 | D | Expired blockhash | Revalidated, re-signed, one eventual outcome. |
| T-REC-04 | D | Transient RPC errors | Backoff, then success or a visible `unknown`. No double spend. |
| T-REC-05 | D | Unfinalized observation later disappears | No receipt from the dropped observation. One later canonical outcome. |

## Solana constraints

| Id | Phase | Case | Pass condition |
| --- | --- | --- | --- |
| T-SOL-01 | D | Wrong token program | `BadTokenProgram`. |
| T-SOL-02 | D | Wrong decimals or wrong mint | `Mint`. |
| T-SOL-03 | D | Alternate signer on release | `Unauthorized`. Seller cannot release. |
| T-SOL-04 | D | One extra token deposited to the vault | Payout and refund still move exactly `principal`. Dust remains. |
| T-SOL-05 | D | SOL fee and rent | Token principal buckets ignore lamports. |
| T-SOL-06 | D | Buyer ATA closed, then refund. Seller ATA closed, then release. | Idempotent ATA create. Funds arrive in the derived account. |
| T-SOL-07 | D | Otherwise valid new fund with zero principal | Program returns `ZeroPrincipal` (6008). No escrow is created and no principal moves. |
| T-SOL-08 | D | Extra token-transfer instruction in the transaction | Signer policy refuses to sign. |
| T-SOL-09 | D | Frozen destination | Transaction fails. Escrow state unchanged. Attempt recorded. |
| T-SOL-10 | D | Same inputs as the sim conformance driver | Outcomes and amounts match the simulator cases. |

## Receipt verification

| Id | Phase | Case | Pass condition |
| --- | --- | --- | --- |
| T-VER-01 | C | Flip one byte of a signed message in the bundle | Exit 2, `invalid_evidence`. |
| T-VER-02 | C | Offline success text | Result is `evidence_signed` and the output does not say `ledger_verified`. |
| T-VER-03 | D | Online check against the local validator | Exit 0, `ledger_verified`, for the payout and both refunds. |
| T-VER-04 | D | Online check with the wrong program id or a failed transaction | Exit 2. |
| T-VER-05 | D | RPC serves the finalized terminal account but lacks a required transaction | Recovery records a gap and issues zero new receipts. Online verification of a previously exported bundle exits 5, `unverifiable`. Contradictory transaction evidence exits 2. |
| T-VER-06 | C | Timeout-before-delivery bundle | Verifies with no verdict signature and no delivery. |
| T-VER-07 | C | Untrusted issuer flag | Exit 3. |
| T-VER-08 | D | RPC URL present only inside the bundle, omitted from the CLI | The verifier does not contact it. Missing `--rpc` on an online request is exit 1. |

## Devnet

| Id | Phase | Case | Pass condition |
| --- | --- | --- | --- |
| T-DEV-01 | E | Payout of test USDC | Finalized release, seller token delta equals principal, online verify exits 0. |
| T-DEV-02 | E | Timeout refund of test USDC | Finalized refund, buyer token delta returns principal, bundle has no invented verdict when the verifier stayed silent. |
| T-DEV-03 | E | Evidence file | Genesis, program id, config, mint, PDAs, signatures, and redacted commands recorded. |

If T-DEV-01 or T-DEV-02 cannot run, phase E writes the blocker and sets the status to `public devnet pending`. The row is not marked passed.

## CI

| Id | Phase | Command | Pass condition |
| --- | --- | --- | --- |
| T-CI-01 | A, extended in C and D | `cargo fmt --all --check` | Exit 0. |
| T-CI-02 | A, extended in D | `cargo clippy --workspace --all-targets -- -D warnings` | Exit 0 on host crates. Program lints follow phase D. |
| T-CI-03 | A | `cargo test --locked -p settle463-core` | Exit 0. |
| T-CI-04 | C | Simulator CLI demos and conformance | Exit 0. No network and no faucet. |
| T-CI-05 | D | Local validator job | Exit 0. Uses `solana-test-validator` started by CI. No public RPC. |
| T-CI-06 | D | Program binary built with the pinned `cargo-build-sbf` | Artifact path recorded. |

CI must not contain private keys other than fixtures, and must not call a public faucet or a public RPC.
