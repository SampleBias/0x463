# Phase D — Local validator conformance

Status target: the financial cases run as real token transfers on `solana-test-validator`, including recovery after a lost RPC response.

Depends on: phase C gate and the phase A toolchain pin.

Blocks: phase E.

## Goal

Implement the Anchor program and the Rust adapter in `04-solana-contract.md`. Run the payout and both refund paths with real signatures and token balances. Show that a restarted process does not double-spend when the RPC response was lost.

## Entry

- Simulator demos still pass.
- `spec/settlement-solana.md` is published.
- The pinned Solana CLI can start `solana-test-validator`.

## Exit gate

1. The program deploys to the local validator and `fund`, `commit_delivery`, `release`, and `refund` match the state table.
2. One finalized payout and both refund paths move exactly `2000000` atomic units of the test mint (T-CLI equivalents on `local-validator`, plus T-SOL-10).
3. T-SOL-01 through T-SOL-09 pass.
4. T-REC-02 passes for a lost funding response and a lost terminal response. Recovery records one deposit, zero receipts while funded, and one receipt after the verified terminal transfer.
5. T-REC-03, T-REC-04, and T-REC-05 pass or, for T-REC-05, a deterministic dropped-fork simulation in the adapter test records `unknown` and then one canonical outcome.
6. Online verification of the payout and both refunds exits 0 with `ledger_verified` (T-VER-03). Tampered program id and a failed transaction exit 2 (T-VER-04). An RPC double that returns "not found" for an old signature exits 5 (T-VER-05).
7. T-TIME boundaries that phase C ran on the simulator are repeated on the validator by warping the validator clock or by using short deadlines and `solana-test-validator` warp. Record which mechanism was used.
8. CI job `validator` passes without a public RPC.
9. `build/phase-d-evidence.md` records the program id, mint, PDAs, signatures, slot numbers, and token deltas.

## Out of scope

Public devnet, Circle USDC, mainnet, and any faucet on the public internet. The mint is created locally and described as a test asset in every receipt.

## WP-D1 — Program

Create `programs/settle463-escrow` with the account layout, seeds, instructions, and errors in `04-solana-contract.md`.

Implementation notes:

- `initialize_config` checks the upgrade authority via the program data account.
- `fund` initializes the escrow PDA and the vault ATA and performs `transfer_checked` inside one instruction so a failed transfer leaves no funded account.
- Store the environment string, not only a hash.
- Use checked arithmetic for amounts. Do not add rent or fees into `principal`.
- On terminal replay, return `AlreadyComplete` before any CPI.
- Transfer `principal`, not the full vault balance.
- Reject `init-if-needed`.
- Keep JCS and schema code out of the program. The program compares 32-byte ids and integers.

Host-compile the program library with the `no-entrypoint` feature if that is what the pinned Anchor version supports, and run clippy with warnings denied. If the program crate cannot be host-clipped cleanly, fix the warnings. Record any lint that the pinned compiler cannot silence without a code change, and prefer a code change.

Build the deployable binary with the pinned `cargo-build-sbf` or `anchor build`. Record the artifact path and the SBPF version.

## WP-D2 — Local fixture

Add `apps/settle463/src/bin` support or a subcommand documented as:

```text
settle463 local-validator prepare --ledger <dir>
```

That command, or a `tests/conformance/validator_setup.sh` used by CI, does all of the following with the pinned Solana CLI:

1. Start `solana-test-validator` on a free port with a fresh ledger directory.
2. Create a six-decimal mint on the classic token program.
3. Create fixture keypairs for buyer, seller, verifier, coordinator, and fee payer. The agent Ed25519 bytes are the Solana keypair bytes.
4. Airdrop SOL only for fees and rent.
5. Mint test tokens to the buyer ATA. Buyer token balance starts at `5000000`.
6. Deploy the program.
7. Call `initialize_config` as the upgrade authority with the mint and the validator genesis hash.
8. Write a local deployment file: genesis, program id, config, mint, token program, RPC URL `http://127.0.0.1:<port>`.

The deployment file is test material. Do not commit a live key. Commit only the script. Generated keys stay under the test ledger directory and are deleted with it.

Refuse to prepare if the mint decimals are not 6 or the mint owner is not the classic token program.

## WP-D3 — Adapter

Implement `SettlementBackend` in `crates/settle463-solana`.

- `prepare_*` returns a decoded review: instruction name, accounts, amount, mint, destinations, deadlines, ids.
- The signer policy from `04-solana-contract.md` runs inside `sign` before any key is used. T-SOL-08 builds a transaction with an extra token transfer and expects a refusal.
- Both buyer and seller review the same bytes for `fund`.
- Persist intent, signed bytes, and the locally derived signature before `sendTransaction`.
- `reconcile` uses `getTransaction` with `finalized` and checks the evidence for the submitted action. Verify funding and delivery again after the escrow has advanced. Cover payout and both refund paths. [Action evidence](../04-solana-contract.md#adapter-reconciliation)
- `inspect` decodes the escrow account.
- Map program error `AlreadyComplete` plus a terminal inspect result to the known outcome.
- Map a frozen-account failure to `FinalizedFailure` with the escrow state unchanged (T-SOL-09). Create the frozen state with the token program's freeze authority in the test.
- Closed ATA case (T-SOL-06): close the empty seller ATA after fund, then release with one idempotent create. Repeat for the buyer ATA and refund.
- Dust case (T-SOL-04): transfer one extra atomic unit into the vault from a third token account, then release, and show the seller received `2000000` and the vault still holds `1`.
- Wrong mint, wrong token program, zero amount, and a seller signature on release return the named program errors.

RPC client traffic uses `rustls` when the chosen Solana client allows it. If the pinned client forces another TLS stack, keep that dependency inside `settle463-solana` and write one sentence in the phase evidence. Domain crates still have no Solana dependency.

## WP-D4 — Shared scenarios

Parameterize the phase C scenario driver by backend. Run:

```text
cargo run -p settle463 -- demo --backend local-validator --scenario pass --deployment <file>
cargo run -p settle463 -- demo --backend local-validator --scenario bad-artifact --deployment <file>
cargo run -p settle463 -- demo --backend local-validator --scenario no-delivery --deployment <file>
cargo run -p settle463 -- demo --backend local-validator --scenario verifier-offline --deployment <file>
```

The stdout contract is the same, with `backend=local-validator` and `asset` set to the mint base58. Deltas are token-account deltas for buyer, seller, and vault.

After each demo:

```text
cargo run -p settle463 -- receipt verify <bundle> --rpc http://127.0.0.1:<port> --trust-file <trust>
```

The trust file contains the issuer, order id, environment, genesis, and program id. The command ignores any RPC URL written inside the bundle (T-VER-08).

Compare each scenario's outcome and principal with the simulator run of the same scenario (T-SOL-10). The comparison fixture lists expected outcome, amount, and whether delivery and verdict are present. It does not compare signatures or slots.

## WP-D5 — Recovery tests

T-REC-02:

1. Run the following sequence for `fund`, then run it for a terminal transfer on an eligible order.
2. Store the signed transaction.
3. Broadcast.
4. Drop the HTTP response in a local proxy or by aborting the client after `send` returns to the proxy and before the proxy returns to the client.
5. Confirm with a separate `solana confirm` or `getTransaction` that the signature landed.
6. Start a new `settle463d` on the same SQLite file.
7. Run the reconciler.
8. For a fund, assert one buyer debit, one vault credit and zero receipts while funded. For a terminal transfer, assert the exact beneficiary credit and one receipt. Reconcile again and assert identical balances and receipt count. Recheck the earlier fund and any delivery against their original transaction signatures after terminalization.

T-REC-03 uses a blockhash that the test expires by waiting or by warping slots, then expects a new signature after revalidation. The old signature is not a second payment.

T-REC-04 points the adapter at a closed port for several attempts, then restores the validator URL. The order ends in one terminal success or stays `unknown` with zero unexpected transfers.

T-REC-05 feeds the adapter a processed-commitment response and then a not-found response for that signature while the escrow account is unchanged. No receipt is written. A later verified terminal transfer produces one receipt.

## WP-D6 — CI

Add a `validator` job that:

1. Installs the pinned Solana CLI and Anchor CLI, or uses the versions already on the runner if the workflow pins them with a checksummed install.
2. Runs `cargo fmt --all --check`.
3. Runs workspace clippy with warnings denied, excluding nothing silently. The program is included to the extent WP-D1 established.
4. Runs `cargo test --locked` including the validator tests. Mark long tests so they run in this job.
5. Builds the program for the deployment target.

The job starts its own validator and creates its own mint. It has no secret, no public RPC URL, and no faucet URL.

## Commands

Record the exact commands that worked, including the validator start command, `anchor build` or `cargo-build-sbf`, deploy, the four demos, and the three verify commands. Also record:

```text
solana genesis-hash --url http://127.0.0.1:<port>
solana program show <program-id> --url http://127.0.0.1:<port>
spl-token balance <buyer-ata> --url http://127.0.0.1:<port>
spl-token balance <seller-ata> --url http://127.0.0.1:<port>
spl-token balance <vault> --url http://127.0.0.1:<port>
```

Use the token CLI that ships with the pinned toolchain. If the binary name differs, record the name that worked.

## Evidence

`build/phase-d-evidence.md` includes:

- Toolchain actually used.
- Program id, config PDA, escrow PDA and bump for the pass order, mint, vault, buyer ATA, seller ATA.
- Funding, delivery, and terminal signatures, plus finalized slots.
- Token deltas.
- T-REC-02 signature that landed before the client learned the result.
- Rent-exempt lamports for the escrow account, and a note that those lamports are not principal.
- Statement `local validator verified`.
- Any test that used a warp helper rather than a natural slot wait.

## Stop conditions

- Tests call the program through `solana-program-test` only. Add the validator path. Program-test may exist as a fast extra, and it does not satisfy the gate.
- A receipt is written from a `confirmed` commitment. Wait for `finalized`.
- The adapter accepts a mint because its symbol metadata says USDC. Require the account owner and decimals.
- Principal moves in the same integer as SOL fees. Split the assertions.

## Handoff

Phase E reuses this program binary and this adapter. It changes the RPC, the mint check, the genesis, and the funding source. It does not change instruction layouts.
