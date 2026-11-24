# Phase F — Handoff

Status target: a clean checkout can be built from the README, and the build report separates simulator, local validator, and devnet.

Depends on: phases A through D complete. Phase E complete or explicitly pending.

## Goal

Leave a repository a new engineer can run and review: README, SDK examples, CI, threat limitations, recovery steps, and a short report of what was actually executed.

## Entry

- Evidence files for phases A through D exist.
- Phase E evidence exists and contains one of the two status lines.

## Exit gate

1. `README.md` at the repository root contains the clean-checkout commands, the trust limitations, and the three status lines.
2. `cargo test --locked` and the CI workflow match the commands in this file on a clean tree. If a full rerun is too expensive, rerun at least the simulator demos, one validator payout, and `cargo fmt` / `clippy`, and say which phase D tests were not rerun.
3. SDK examples for the buyer, the seller, and the verifier run against the simulator.
4. `build/REPORT.md` lists commands, outcomes, limitations, and pending external verification.
5. The report does not say the protocol is production ready and does not claim an audit.
6. No private key, faucet credential, or RPC API key is committed. `git grep` for the fixture seed domain is limited to `fixtures/` and tests.

## WP-F1 — Root README

Write `README.md` for an engineer who has not read the plan. Keep it concrete. Required sections:

1. What v0.1 does, in one page. Include the brand-string rule and the sentence that no LLM is required.
2. Trust. The verifier can approve bad work. An unavailable verifier causes a refund at `resolve_by`. The coordinator cannot move principal. A local audit hash chain can be replaced by whoever controls the database. Offline verification does not prove finality. The devnet program upgrade authority can change the program.
3. Toolchain, pointing at `spec/toolchain.md`.
4. Build and test:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --locked
```

Adjust the clippy line if phase D documented a program-crate exception. Quote the exception.

5. Simulator demos, exactly the four `cargo run -p settle463 -- demo --backend sim --scenario ...` commands, plus `order inspect` and `receipt verify --offline`.
6. Local validator: how to start the validator, create the test mint, deploy, and run the same scenarios. State that the mint is a test asset.
7. Devnet: either the redacted commands and the recorded addresses, or the pending blocker from phase E. Do not invent signatures.
8. Recovery: intent is stored before broadcast; restart runs reconcile; a lost response is not a second payment; an archival gap is `unverifiable`.
9. Layout of crates and `spec/`.
10. Status block with the three labels and the git revision.

Point readers at `spec/README.md` for normative field rules. The README summarizes and does not fork the specification.

## WP-F2 — SDK examples

Add and run:

```text
cargo run -p settle463-sdk --example buyer_sim
cargo run -p settle463-sdk --example seller_sim
cargo run -p settle463-sdk --example verifier_sim
```

Each example uses public client types, fresh fixture keys, and the simulator or the local HTTP service. The buyer example ends having read the artifact bytes or having exported a receipt. The seller example publishes an offer and a quote. The verifier example signs a PASS for the reference bytes and refuses a one-byte mutation. Document the commands in the README.

These examples are the public SDK. A markdown file that shows pseudocode without a compiling example does not finish this work package.

## WP-F3 — CI completeness

`.github/workflows/ci.yml` runs:

- `cargo fmt --all --check`
- `cargo clippy` with warnings denied on the supported host targets
- `cargo test --locked`
- golden vector check and `python3 fixtures/tools/independent_vector.py`
- simulator end-to-end tests
- the local-validator job from phase D
- the program build for the deployment target

No job contacts public RPC or a faucet. Devnet is absent from CI.

## WP-F4 — Report

Write `build/REPORT.md` with:

| Section | Content |
| --- | --- |
| Commands | Every gate command from phases A through F that was run, with exit code. |
| Outcomes | Simulator status, local validator status, devnet status. |
| Artifacts | Paths of the golden vector, the four sim receipts, the validator signatures, and the devnet receipts if any. |
| Limitations | Verifier honesty and availability, coordinator availability, upgrade authority, no audit, no mainnet, v0.1 party and fee limits, relay outage can cause refund. |
| Pending | The phase E blocker if any, plus anything in the conformance matrix that was not run. |
| Divergence | Any plan amendment made during implementation, with the reason. |

Use the status phrases from `plan/README.md` only.

## WP-F5 — Clean-tree check

In a temporary directory, clone or copy the repository without `target/` and without `build/` evidence, then follow the README from a cold start through one simulator pass demo and an offline verify. If the copy still needs a command that the README omits, add that command to the README and rerun.

Do not commit `target/`, ledger directories, SQLite files, or keypairs generated by the demos. Add the appropriate `.gitignore` entries.

## WP-F6 — Plan reconciliation

Skim `plan/03-conformance-matrix.md`. Every row is `passed` with a pointer to an evidence file, or `pending` with a blocker. Add that checklist as an appendix to `build/REPORT.md`. Do not delete the plan. Future changes to the wire format start by editing the plan and the spec together.

## Commands

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --locked
cargo run -p settle463-sdk --example buyer_sim
cargo run -p settle463-sdk --example seller_sim
cargo run -p settle463-sdk --example verifier_sim
cargo run -p settle463 -- demo --backend sim --scenario pass
```

Plus the clean-tree copy's simulator demo.

## Evidence

`build/phase-f-evidence.md` points at `build/REPORT.md` and records the clean-tree command log. `build/REPORT.md` is the document a reviewer reads.

## Stop conditions

- The README tells the reader to trust a receipt's embedded RPC URL. Remove that instruction.
- The report marks devnet verified without phase E signatures. Change it to pending.
- Examples bypass the SDK by calling private modules. Rewrite them.

## Done

When this gate passes, the handoff is complete for v0.1. Further work — package publication, public hosting, mainnet, a second verifier profile, fees — needs a new authorization and a new plan.
