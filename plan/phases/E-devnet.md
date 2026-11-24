# Phase E — Public devnet

Status target: one test-USDC payout and one test-USDC timeout refund on Solana devnet, or an explicit pending status with the blocker.

Depends on: phase D gate.

Blocks: the devnet section of phase F. Phase F still ships if this phase is pending.

## Goal

Run the reference trade against Circle's devnet USDC mint using the program and adapter from phase D. Record evidence that an independent person can re-verify. Keep test keys and test funds clearly labeled.

## Entry

- Phase D evidence says `local validator verified`.
- A test-only deployer keypair exists outside the repository. It is not a personal long-term wallet and it is not committed.
- The operator has approved contact with public devnet RPC, a public faucet, and Circle's address list. CI has not approved that contact. This phase is manual and opt-in.

## Exit gate

Either:

1. T-DEV-01 and T-DEV-02 pass, T-DEV-03 is written, online verification exits 0 for both bundles against the trusted devnet RPC, and the status line is `public devnet verified`, or
2. The run stops at a named blocker, no devnet receipt is presented as success, and the status line is `public devnet pending`.

Simulator or local-validator transcripts do not satisfy this gate.

## Out of scope

Mainnet, mainnet USDC, customer funds, program publication to a public registry beyond the devnet deploy required for the test, and CI execution of this phase.

## WP-E0 — Preflight before any signature

Write the results into `build/phase-e-preflight.md` before `fund` is signed.

1. Fetch `https://developers.circle.com/stablecoins/usdc-contract-addresses` and read the Solana devnet row. The planning reference is `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`. If the page and the reference differ, use the page and record both values. If the page cannot be fetched, stop with blocker `circle address list unavailable`.
2. Query the chosen devnet RPC for that mint. Record owner, decimals, and supply. Continue only when the owner is `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` and decimals are 6.
3. Query genesis hash. Record it.
4. Confirm the deployer's upgrade authority and that the key is a test key. Record the public key only.
5. Confirm the buyer has devnet SOL for fees and devnet USDC for `2000000` atomic units plus a buffer. Use Circle's published faucet or an equivalent documented faucet. If the faucet refuses or the buyer balance is short, stop with blocker `test funds unavailable`. Do not buy tokens and do not bridge mainnet funds.
6. Confirm the RPC is one the operator configured in the shell or a local config file. A URL inside a receipt is not an input.

Suggested RPC values are the operator's choice. Record the host that was used. Do not vendor an API key into the repository. If every public RPC fails, the blocker is `devnet RPC unavailable`.

## WP-E1 — Deploy

1. Build the phase D program with the same toolchain recorded in phase D. If devnet rejects the SBPF version, rebuild with the architecture devnet accepts, record the new flag, and note that local-validator and devnet binaries differ.
2. Deploy with the test upgrade authority.
3. Run `initialize_config` with the Circle mint and the RPC genesis hash.
4. Write `build/devnet-deployment.json` containing genesis, program id, config PDA, mint, token program, upgrade authority public key, and the environment string. Redact keypaths. Do not write private keys.
5. The adapter must refuse to sign if this file's genesis, program id, or mint disagrees with live RPC reads.

There is no instruction to change this config afterward. Say that in the evidence file.

## WP-E2 — Payout

Run the pass scenario with fresh devnet party keys:

```text
cargo run -p settle463 -- demo --backend devnet --scenario pass --deployment build/devnet-deployment.json
```

The flow is the same SDK and CLI path as phase C and phase D: offer, request, quote, accept, fund, upload, deliver, PASS, release.

Deadlines must be long enough for devnet confirmation and short enough to finish in one session. Suggested offsets from the observed slot time: `fund_by` at least 30 minutes ahead, `deliver_by` at least 30 minutes after `fund_by`, `resolve_by` at least 30 minutes after `deliver_by`. Record the absolute values. Authoritative checks still use the Clock sysvar.

After the CLI prints the receipt path:

```text
cargo run -p settle463 -- receipt verify <bundle> --rpc <approved-endpoint> --trust-file <trust>
```

Then confirm with the Solana CLI, against the same RPC, that the terminal signature is finalized and that the seller token balance increased by `2000000`.

## WP-E3 — Timeout refund

Use a second order. Deliver the artifact or skip delivery. The required case is a timeout refund, and the silent-verifier shape is the one to run if time allows one case only: commit delivery, do not publish a verdict, wait until `resolve_by`, and refund.

```text
cargo run -p settle463 -- demo --backend devnet --scenario verifier-offline --deployment build/devnet-deployment.json
```

If waiting for `resolve_by` is impractical, a dedicated order with deadlines a few minutes apart is allowed when the signed quote contains those absolute deadlines and the program enforces them. Record that choice. Do not warp devnet.

Verify the bundle online. The bundle must not contain a verifier signature that was never produced. Buyer token balance, net of this order, returns the principal. Fees in SOL may have decreased.

## WP-E4 — Evidence and reproduction

Write `build/phase-e-evidence.md` with:

- Date, git revision, toolchain.
- Circle page URL and the mint that was checked.
- Genesis hash, program id, config PDA, mint, upgrade authority.
- For each order: order id, escrow PDA, vault, buyer ATA, seller ATA, amount, outcome.
- Funding signature, delivery signature if any, terminal signature, finalized slot.
- `receipt verify` exit code and result label.
- Redacted commands. Replace home directories and key file paths with placeholders.
- Status line `public devnet verified` or `public devnet pending`.
- Blocker text when pending. Name one of: `circle address list unavailable`, `mint mismatch`, `devnet RPC unavailable`, `test funds unavailable`, `deploy rejected`, `toolchain cannot build for devnet`, or a more specific error quote from the tool.

Also copy the two bundles into `build/devnet-receipts/` when the runs succeed. They contain public keys and transaction signatures, not private keys.

## Stop conditions

- The mint on devnet is the mainnet USDC address. Stop.
- Decimals are not 6 or the token program is not classic SPL Token. Stop.
- A command would sign with a key that is not in the test deployment file. Stop.
- The only successful transcript is from `--backend sim` or `--backend local-validator`. Do not copy it into the devnet evidence as a success.

## Handoff

Phase F prints the status line from this phase verbatim. If the status is pending, the README's devnet section is the reproduction procedure plus the blocker, and it does not show sample devnet signatures.
