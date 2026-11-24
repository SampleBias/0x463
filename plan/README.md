# 0x463 build plan

Version 0.1.0 planning set. 24 November 2026.

This directory is the build procedure for the 0x463 reference implementation. Execute it from phase A through phase F. The checkout was empty when this plan was written. These files are the planning handoff. They are not the normative `spec/` tree, not the SDK, and not a claim that any gate has been run.

The product is an agent-to-agent settlement protocol. A buyer and a seller sign terms. The buyer funds escrow. A designated verifier checks one precommitted artifact. The escrow pays the seller on a timely PASS or returns the principal under the timeout rules. Each terminal outcome produces a verifiable receipt. Release is the irreversible payment. No LLM is required.

`0x463` is a brand string. It is never a chain identifier, an HTTP status, an asset ticker, a wallet address, or a signature secret. The Rust prefix is `settle463`.

## How to execute

1. Read this file, then `00-locked-decisions.md`, `01-wire-contract.md`, `02-api-persistence.md`, `04-solana-contract.md`, and `03-conformance-matrix.md`.
2. Implement one phase at a time, in letter order. The work packages inside a phase run in the order written.
3. Run the phase gate commands. Record the evidence the phase asks for before starting the next phase.
4. Use the public SDK and the `settle463` CLI for end-to-end flows. A design document, a crate skeleton, or a test that calls private helpers instead of the SDK and CLI does not pass a gate.
5. If a locked name, byte layout, or error code must change, amend the plan file in the same change and state why. Do not let the code drift ahead of the plan.
6. Stop at the end of phase E if devnet cannot be executed. Record the blocker there. Finish phase F with devnet marked pending. Simulator evidence is never a substitute for devnet evidence.

## Phase index

| Phase | File | Exit gate |
| --- | --- | --- |
| A | [phases/A-specification-and-fixtures.md](phases/A-specification-and-fixtures.md) | Spec, workspace, and golden vectors. A one-byte or one-signature change is detected. An independent implementation reproduces the golden ids. |
| B | [phases/B-negotiation-and-simulator.md](phases/B-negotiation-and-simulator.md) | Two agents with distinct keys negotiate and fund on the simulator through the SDK. Expired quotes, wrong networks, excessive prices, and competing acceptances cannot create a second funding authorization. |
| C | [phases/C-fulfillment-receipts-and-service.md](phases/C-fulfillment-receipts-and-service.md) | The four simulator scenarios run from the CLI. A fresh process verifies each receipt. The buyer reads the accepted fixture through the scoped client. |
| D | [phases/D-local-validator.md](phases/D-local-validator.md) | The same financial cases run against a local validator and a six-decimal test mint. One payout and both refund paths finalize. Restart after a lost RPC response does not double-spend. |
| E | [phases/E-devnet.md](phases/E-devnet.md) | A devnet payout and a devnet timeout refund, with recorded addresses and verification, or an exact blocker and the status `public devnet pending`. |
| F | [phases/F-handoff.md](phases/F-handoff.md) | Clean-checkout README, SDK examples, CI, trust limitations, recovery instructions, and a build report that separates simulator, local validator, and devnet status. |

## Dependency graph

```text
A --> B --> C --> D --> E --> F
              \--> D
```

Phase D reuses the phase C scenarios and the phase A wire contract. Phase E reuses the phase D program and adapter. Phase F packages the evidence from A through E.

## Definition of done

The implementation is done for this handoff when every row in [03-conformance-matrix.md](03-conformance-matrix.md) is either passed with a command and a result, or explicitly marked blocked with the missing external dependency. The final report uses only these status words:

- `simulator verified`
- `local validator verified`
- `public devnet verified`
- `public devnet pending`

Do not write "production ready". Do not claim a security audit. Do not describe hosted verification, usage fees, mainnet, or a live customer asset as part of this delivery.

## What this delivery contains

- Runnable Rust workspace described in phase A.
- Open specification under `spec/`, written during phase A from the contracts in this plan.
- SDK clients for the buyer, the seller, and the verifier.
- CLI demos and receipt verification.
- Simulator, local-validator adapter, and a Solana devnet USDC adapter.
- Conformance tests and golden vectors.
- `Cargo.lock` committed.
- Build and test report written in phase F.

## What this delivery excludes

Mainnet, live customer funds, centralized exchanges, proprietary tokens, price oracles, cross-chain atomic swaps, governance tokens, fractional payouts, reputation, subjective disputes, arbitrary verifier plugins, executable user code, production hosting, package publication, and domain purchase. Version 0.1 is one buyer, one seller, one verifier, one artifact, one fixed-price transfer, and zero protocol fees.

## Repository map to create

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
rustfmt.toml
clippy.toml
README.md
spec/
fixtures/golden/positive-signing-vector.json
fixtures/artifacts/
crates/settle463-core/
crates/settle463-verify/
crates/settle463-sim/
crates/settle463-solana/
crates/settle463-sdk/
apps/settle463d/
apps/settle463/
programs/settle463-escrow/
tests/conformance/
.github/workflows/ci.yml
```

Domain crates (`settle463-core`, `settle463-verify`, `settle463-sim`, `settle463-sdk`) compile with no Solana dependency. Only `settle463-solana` and `programs/settle463-escrow` depend on Solana. The CLI may depend on both adapters.

## Evidence rule

Each phase ends by writing `build/phase-<letter>-evidence.md`. That file records the date, the git revision, the exact commands, the exit codes, and the paths of receipt bundles or transaction signatures. Phase F collects those files into `build/REPORT.md`. Do not invent a result that a command did not print.

## Planning fixtures already in this directory

[golden/positive-signing-vector.json](golden/positive-signing-vector.json) is an independent vector. It was produced with Python `hashlib` and `cryptography`, then checked with OpenSSL 3.6.5 `pkeyutl -verify` on the planning host. Phase A copies it to `fixtures/golden/` and checks the Rust implementation against it. Regenerating it with the Rust crate under test does not satisfy the independence requirement.

The reference artifact is the 52-byte UTF-8 value with no trailing newline:

```text
{"job":"sample-001","result":"complete","units":"3"}
```

SHA-256: `37ba5c2e570e4e256af7049d0d9c27f70b50f5de4210101d7c0bd2b497db7587`.
