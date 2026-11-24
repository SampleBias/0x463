# Phase D local-validator evidence

**local validator verified** on revision `55c45780c33856baaae211a8cd40e2058d924df9`. The private validator finalized a payout and both timeout refund paths for 2,000,000 atomic units of a locally created six-decimal test asset. Each order had its own buyer and seller token accounts. [Validator job](https://github.com/SampleBias/0x463/actions/runs/45465171449/job/138698040114) · [Public evidence archive](https://github.com/SampleBias/0x463/actions/runs/45465171449/artifacts/14955358242)

## Deployment

The runner used host Rust 1.89.0, Anchor 1.2.2, Agave 4.1.2, SPL Token CLI 5.6.1 and cargo-build-sbf 4.1.0. The build command selected platform-tools v1.57 and SBPF v3 explicitly. The SHA-256 of `$RUNNER_TEMP/escrow/settle463_escrow.so` was `53d51a217cae0afb221411fcf46ad5d486b8a4a728a003fd4b25f292b8e28807`. The adapter uses rustls for its RPC client.

| Value | Observed on the validator |
| --- | --- |
| Genesis | `2YrwS6QYaKvFwBQhKDna4BnvxYoGVsyWQHpHSnzf7CtE` |
| Program ID | `3CEGonqu1gQEsT2vJGfesoW1p25NcYuRp7McC4yFSrbd` |
| Config PDA | `48a84rdZxEEiLjXjiPgch9ZhijiWssn3t2a7fxvfeDeT` |
| Classic SPL Token program | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` |
| Local test mint | `7UXdrn2UaDuUktYDQDFYo3hk39ow2UjQfHjnF6FfQmc1` |
| Upgrade signature | `4GxzTYKPo6c7HQ652oDLSNWte87FXfdxBZFtafXBQHr3gMtkMLjn6oPQ3Du78CmdgnNWJHEhJ6CsU1RTYZ85p22k` |
| Upgrade slot | `73` |
| Initialize config signature | `4WbhmQiADZucgbNfN4n7uDa25mvEVHhYGqsecwoGopWqWp9nJzNCSWDt313gPRMPqWfzusDtUpC89SD43sUsvLoo` |
| Initialize config slot | `268` |
| Pass order escrow PDA | `CgmirETqRNxR5pTpx8kQTdMTKtpFBmRpmk4qCJwgMRF` |
| Escrow bump | `254` |
| Vault ATA | `2PLCXzCW6w4Rf5HhNocyB7ypttDUvdcWn7EjixXoUPAK` |
| Buyer ATA | `9UVnGtBJAEmMXMYjrj8uGJoSYyrjoLfuJbHqZxm5UqqB` |
| Seller ATA | `8Vr9Tuzheq7gN7PEVaL6iTsqvv9HpwrfsnfYkHsdQG8i` |
| Rent exemption for 698-byte escrow (lamports) | `5748960` |

The upgrade authority deployed the built binary and initialized the mint and genesis pins. A separate program test measured the buyer’s SOL debit for escrow rent, vault rent and transaction fee. All principal and balance figures below use SPL atomic units.

## Finalized orders

Each buyer opened with 5,000,000 units. Funding moved 2,000,000 units to escrow in each case. All four CLI scenarios matched their simulator counterparts and produced `evidence_signed` offline and `ledger_verified` online.

| Scenario | Outcome | Buyer net | Seller net | Vault net |
| --- | --- | ---: | ---: | ---: |
| pass | `paid` | -2000000 | 2000000 | 0 |
| bad-artifact | `refunded_timeout_after_delivery` | 0 | 0 | 0 |
| no-delivery | `refunded_timeout_before_delivery` | 0 | 0 | 0 |
| verifier-offline | `refunded_timeout_after_delivery` | 0 | 0 | 0 |

### pass

Order `318c447dea6d58eb25a9944cd0dc6c8eba03bb0e8bea005dac0d267c03c37c5b`, escrow `CgmirETqRNxR5pTpx8kQTdMTKtpFBmRpmk4qCJwgMRF`, vault `2PLCXzCW6w4Rf5HhNocyB7ypttDUvdcWn7EjixXoUPAK` and mint `7UXdrn2UaDuUktYDQDFYo3hk39ow2UjQfHjnF6FfQmc1`.

| Instruction | Finalized slot | Signature |
| --- | ---: | --- |
| fund | 308 | `5rx2XP1tNKj4rgA3CMDT6T3VNHP9jWzWj82Me8769k83U6BCaDduBrrFBJtTrwqNrJVQ7DVT8JMnpLwBVyU7uHFx` |
| commit_delivery | 344 | `5hrWfTJUzYbWCqHTdBUDZgQv97Hn1rS3JBGpbEpF6ba6H6QKcvshaGYuraUDRZ3cCWWZMNkuAqAvQq4cadi48yWi` |
| release | 387 | `2LurRZqV9h5N35Hm4TeUBee6w8Gb1y8Ta5vFMQDqdLrDviGFdMbuuj5irCYH61WJ46r9MyEQLdxZP6UfSCa3iauy` |

### bad-artifact

Order `45b77a89cf96689e82a850149d76cbb4fd63979014d55f53da5cd9e4a3caade8`, escrow `7osuLWG6nJZGDmv7KPKRKH4KddsvookAGspBGLHb5qtu`, vault `EyTdstQsYLVUVZE2Ra9DzLvYbZy3RbqaHffWgTTnBbJQ` and mint `7UXdrn2UaDuUktYDQDFYo3hk39ow2UjQfHjnF6FfQmc1`.

| Instruction | Finalized slot | Signature |
| --- | ---: | --- |
| fund | 338 | `5notoPmgWYyHNNiE8ppz6HZAGKNwXQX2gmrJUkrEp7LHtYJy3YSWsEBkZxZqgSomZzPcLGKiA689Sxnf8d1qMM83` |
| commit_delivery | 371 | `3nicUvjEYvFjA7N115RJiGucTkyL2hjCsRF3naADHXtkFxqYpQ7CKf4bizVHQ4g6R9Xbste8TTXkCG1gtPTtAuq4` |
| refund | 494 | `2eR32ufvxNWczF2jPUpSUj8nSMbpGjGy3YidXZhhVE24xmQbVUEuKuoD57mKQvCPYophy2gkvh8UbxEcdZfSqHut` |

### no-delivery

Order `613b2ab06ab2be281ba14c09747949e0062dd8dd2299fcbbb0f818dc24ec6bfc`, escrow `CDVKUxs75wFktPCk4eE7URRjmpV5ocrBVMroUXmofbBp`, vault `Hvkcu16qb54WU1LkJRbmB9sJ8Zw95sW4QJ8u2AxrKuwc` and mint `7UXdrn2UaDuUktYDQDFYo3hk39ow2UjQfHjnF6FfQmc1`.

| Instruction | Finalized slot | Signature |
| --- | ---: | --- |
| fund | 336 | `4eHxneipCMM1KFxnRMSinp5R45nEubxESzFFwTP3TDqe1JBFTLdbk6UT4zvpnJtQ8BoJ2rLEViupR9ipx29tVN5` |
| refund | 433 | `8Z321SVVf7ZGiaVDpK5s34awVAUryuKurv39Q1exEs1g8mQUS7dUpC77wrXM3cb8As1EkACFWoMvuem8WMdbuXY` |

### verifier-offline

Order `2a1d216f93427e9c65106f2b0063d20bcc02ccc62d18d4afefb8ad7118578eb2`, escrow `BjE7xmPV5gU8KZnjdnnvcgMHB83D8QfH3vMJ2M2GE9cf`, vault `HgBz3mywek2C9bDE7zSUPjXvrx5ZwEP1AWfpsPZviQqz` and mint `7UXdrn2UaDuUktYDQDFYo3hk39ow2UjQfHjnF6FfQmc1`.

| Instruction | Finalized slot | Signature |
| --- | ---: | --- |
| fund | 337 | `5BrL6LmTseArz9Tjhe5fNUdgGiq5DGHdRtLN7U6Zs4UqGVsMpQyC6Se2UknwLXyuqcKyrEVDqGmPuJQ8XDpFvh9G` |
| commit_delivery | 370 | `2MwtsTub54hwoGFTrHcqsqnSmVPoCimh3d89YQ9gUi3Pqpd8EgoooybQnSjqKaX5JeKGfLGXvZkBSwscjvsJiDiR` |
| refund | 494 | `4RkmRA2iUsNpHeV65QaLVfodBnkeTHmMpgTFEAYHWQUd3WDibij5Hxq3yDHZDMChX5Pz532nkFD3Ns7V1M1V1Lov` |

## Recovery and receipt verification

The proxy interrupted the coordinator before broadcasting a signed release and after the validator accepted funding and release transactions. The test inspected each order twice through fresh CLI processes on the same SQLite database. Both restarts produced the balances and receipt count shown for each row.

| Interruption | Instruction | Finalized slot | Signature | Buyer / seller / held / receipts |
| --- | --- | ---: | --- | --- |
| lost_response | fund | 337 | `5zWcVgPhPzoDZ3KNCWSZnN8NGkfW7y756mv1rVarC6K9VKNzJCAN4eV65xpdAscr1fzEHL3Ye6W2DLR63Ph3FU7G` | 3000000 / 0 / 2000000 / 0 |
| before_broadcast | release | 414 | `25cAb2BmG2rdX5KZuh9Qnf7SEFsX3GPjiUkLuKafHMoarcVdSBeUm24J4Q4BjNKU42imRsuzUnwMdKMQ1Dmm1Fu7` | 3000000 / 2000000 / 0 / 1 |
| lost_response | release | 415 | `5yb9ZbfD3Ku3L8mHGZ2yuZ1fpNKoWh5mR7sjgGju46Mym5QKCJL1RSyYi5yZEvw1WASuMu7hesejDgyb5cNNhzRk` | 3000000 / 2000000 / 0 / 1 |

For blockhash expiry, the first signed funding attempt `3eDhn8Esop1yYmWwxyBvZhiPemFnhjrfBktXpCwLURx2CEi5oRSSzGPQNGG5D6YUzvv7DQZsjdrG9qkcb5fzAWUb` expired at block height 467. Both funding parties reviewed the replacement. The restored RPC finalized `3rX1dP17YMkertmpt25nN5i9x6yJWK6S9NxGDxJAhREUpMmTv8GANNnBXcxjkns1GyqYdKSV2fULgzg4cvVUcmsP` at slot 504. The escrow’s finalized history contained 1 transaction. During the closed-port test the original signed bytes remained available for retry.

The adapter's deterministic fork test observed a provisional release, its disappearance, and one canonical payout. The live release recovery test temporarily hid the original funding transaction after the escrow became terminal. The coordinator stayed responsive, recorded a gap and held the receipt until history returned.

The payout verifier rejected a mismatched program pin, a changed ledger program and a failed terminal transaction with exit 2. Missing finalized transaction history returned exit 5. A receipt-supplied RPC hint received zero requests. An online verification without a caller-supplied RPC exited 1.

An independently submitted version-0 refund with a separate transaction payer was recovered and verified online. The finalized signature was `3vY2Ensyaakyr8YKeAhUPTRhif4e69QR5WzQyygKkaaPREqBcRKZweFtK9GTeKHt1kVw9mq4N2NJaNEWK7pHNtCt` at slot 466. [Validator job](https://github.com/SampleBias/0x463/actions/runs/45465171449/job/138698040114)

## Program and clock checks

Wrong token program, wrong mint, zero principal and an unauthorized release signer returned the specified program errors. Terminal replay performed zero inner instructions. A frozen destination failed at signature `2RwRq7wyVvFF8wz2Ad5U8oLSX2BwY7LhFsE4U6iYyL6sFsuXR8qJgrexbm6f4WfFCcCnfvXmScvPvLhCthBBDjg4` with escrow state and token balances preserved. Payout and refund recreated a closed destination ATA and left one unit of third-party dust in each vault. The signer-policy tests rejected an additional transfer before signing.

The validator advanced naturally. The deadline test sampled six cases apiece at one second before, at, and one second after the boundary. It checked the Clock sysvar at the simulated slot. The release signed before the deadline and submitted late failed at `5MCrB5exVTBg7tjxm9LNvTfLpnCvDTkN4Wqa6sTX7k1GgVuTe5oc7QNcJm5bp32q5DzWV8ZjrDjmnaqpUsf7v32T`. Refund `5DwjEoJfHR6cDvLYwTuM7hj9Qnp5n9jds8T2ayGf2fUePfoDKgD3pw7Pd9PpWSpsNaV5JdLqx5i1Cxom1pJsvx3D` succeeded.

## Reproduction

The workflow installs and checksums the pinned toolchain, provisions SQLite 3.54.1, and installs solders 0.26.0 and cryptography 50.0.2. These commands run from the repository root in that environment. [Workflow](../.github/workflows/validator.yml) · [Fixture](../tests/conformance/validator_setup.sh)

```bash
cargo-build-sbf --tools-version v1.57 --arch v3 --manifest-path programs/settle463-escrow/Cargo.toml --sbf-out-dir "$RUNNER_TEMP/escrow" -- --locked
cargo build --locked --offline -p settle463 -p settle463d
bash tests/conformance/validator_setup.sh "$RUNNER_TEMP/escrow/settle463_escrow.so" \
  python3 tests/conformance/validator_scenarios.py "$RUNNER_TEMP/validator-evidence"
```

The fixture selects a free loopback port, preallocates the program under its generated upgrade authority, then deploys the built binary. The scenario driver runs the following CLI commands for each independently funded buyer, using that buyer’s deployment file and new session directory. Account queries apply while the fixture is running.

```bash
solana-test-validator --ledger "$fixture/ledger" --bind-address 127.0.0.1 \
  --rpc-port "$port" --faucet-port "$((port+2))" --mint "$payer_pubkey" \
  --upgradeable-program "$program" "$program_so" "$payer_pubkey"
solana --url "$rpc" --keypair "$payer" --commitment finalized program deploy "$program_so" --program-id "$program" --use-rpc --output json
# Each invocation receives scenario=pass, bad-artifact, no-delivery or verifier-offline.
target/debug/settle463 demo --backend local-validator --scenario "$scenario" --deployment "$deployment" --output-dir "$session"
target/debug/settle463 receipt verify "$session/receipt.json" --offline --trust-file "$session/trust.json"
target/debug/settle463 receipt verify "$session/receipt.json" --rpc "$rpc" --trust-file "$session/trust.json"
solana genesis-hash --url "$rpc"
solana program show "$program" --url "$rpc"
for ata in "$buyer_ata" "$seller_ata" "$vault"; do
  spl-token balance --address "$ata" --url "$rpc"
done
```

The disposable ledger and fixture signing keys are deleted at the end of the run. The retained archive contains public receipts, detached trust inputs, transcripts and ledger observations.
