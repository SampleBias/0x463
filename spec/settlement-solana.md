# Solana settlement

The v0.1 program is `settle463-escrow`. The Rust adapter is `settle463-solana`.

Classic SPL Token program: `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`.

Associated token program: `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`.

System program: `11111111111111111111111111111111`.

Compute budget program: `ComputeBudget111111111111111111111111111111`.

Use checked SPL transfers. Use the Anchor version pinned in phase A. Do not enable `init-if-needed`. Destination account recreation uses the associated-token idempotent create instruction as its own instruction, and the signer policy allows that instruction only for the precommitted account.

## Deployment configuration

Config PDA seeds, in order:

```text
b"settle463"
b"config"
b"v1"
```

`initialize_config` may be signed only by the program's upgrade authority, checked against the program data account. The instruction initializes the config once. There is no update instruction and no withdraw instruction. A later caller cannot replace the mint, the token program, or the recorded genesis hash.

Config fields:

| Field | Rule |
| --- | --- |
| `bump` | PDA bump. |
| `mint` | The only mint this deployment accepts. |
| `token_program` | Must equal the classic token program. |
| `decimals` | Must equal `6` and must equal the mint account's decimals. |
| `genesis_hash` | 32 bytes attested by the upgrade authority at init. |

Clients read genesis from their trusted RPC and refuse to sign when it differs from `genesis_hash`. The program cannot read the cluster genesis itself. A malicious upgrade authority can change the program later. That is the devnet upgrade-authority assumption, and the README states it. Local validator tests may use the validator's upgrade authority. Production multisig and a frozen upgrade authority are out of scope.

The environment string is `solana:<genesis-base58>:<program-base58>:<config-base58>` using the RPC genesis, the deployed program id, and this config PDA.

## Escrow PDA

Seeds, in order:

```text
b"settle463"     // 9 bytes
b"escrow"        // 6 bytes
b"v1"            // 2 bytes
buyer_pubkey     // 32 bytes
request_nonce    // 32 bytes
```

Every seed is at most 32 bytes. The bump is stored on the account. The order id is not a seed. The same buyer and nonce always produce the same PDA on a given program, which is the on-chain form of the single-deposit rule.

The vault is the associated token account of the escrow PDA for the config mint and the classic token program. Its owner is the escrow PDA. The program derives this address and rejects a caller-supplied substitute.

The buyer refund account is the ATA of the buyer wallet. The seller payout account is the ATA of the seller wallet. Both are derived and stored. The quote's settlement object must contain the same addresses before either funding signer signs.

## Account data

Anchor discriminator plus the following fixed fields. Store the environment string losslessly: a `u8` length and up to 141 bytes. The maximum Solana environment length is 141 (`solana:` + three 44-character base58 values + two colons). Reject a longer environment in `fund`.

| Field | Width |
| --- | --- |
| `order_id` | 32 |
| `request_id` | 32 |
| `quote_id` | 32 |
| `acceptance_id` | 32 |
| `policy_hash` | 32 |
| `buyer` | 32 |
| `seller` | 32 |
| `verifier` | 32 |
| `mint` | 32 |
| `buyer_destination` | 32 |
| `seller_destination` | 32 |
| `vault` | 32 |
| `token_program` | 32 |
| `config` | 32 |
| `principal` | u64 |
| `fund_by`, `deliver_by`, `resolve_by` | i64 each |
| `state` | u8 |
| `delivery_id` | 32 |
| `delivery_set` | u8 |
| `verdict_id` | 32 |
| `verdict_set` | u8 |
| `bump` | u8 |
| `environment` | length byte plus bytes |

State values: `Funded = 0`, `Delivered = 1`, `Paid = 2`, `Refunded = 3`. There is no on-chain `Unfunded` account. `fund` creates the account already in `Funded`.

Publish the `InitSpace` calculation in the program comments and record the rent-exempt lamport cost in the phase D evidence. Rent is paid by the transaction fee payer in SOL. It is not deducted from principal.

JCS parsing, artifact retrieval, and schema validation stay off-chain.

## Instructions

Instruction names, which determine Anchor discriminators: `initialize_config`, `fund`, `commit_delivery`, `release`, `refund`.

### `fund`

Signers: buyer and seller. The transaction fee payer is the buyer.

Preconditions:

- `now < fund_by < deliver_by < resolve_by`
- `principal > 0`
- buyer, seller, and verifier pubkeys are pairwise distinct
- mint, decimals, and token program match config
- derived escrow PDA, vault ATA, buyer ATA, and seller ATA match the instruction accounts and the stored addresses
- buyer token account mint and owner match
- environment bytes match the client's pinned environment

Effects, one transaction: create the escrow account, create the vault ATA if needed, checked-transfer exactly `principal` from the buyer token account to the vault, set state `Funded`, store every id, key, destination, deadline, and the environment.

### `commit_delivery`

Signer: seller, and that seller equals the stored seller.

Preconditions: state `Funded`, `now < deliver_by`, `delivery_set == 0`.

Effects: store the 32-byte delivery message id and set `delivery_set = 1`. State becomes `Delivered`. No token movement.

The seller's Solana signature covers the instruction data. The adapter still requires a valid off-chain `delivery` message whose message id equals that 32-byte value before it asks the seller to sign.

### `release`

Signer: the stored verifier. A distinct fee payer may pay the network fee and is not recorded as an authority.

Preconditions: state `Delivered`, `delivery_set == 1`, the instruction's delivery id equals the stored delivery id, `now < resolve_by`.

Effects: checked-transfer exactly `principal` from the vault to the stored seller destination, store the PASS report id, set `verdict_set = 1`, set state `Paid`.

The program does not parse the verdict JSON. The verifier process is the component that refuses to sign this instruction unless the PASS report is the one named here. A dishonest verifier can still sign. That limitation stays documented.

### `refund`

Signer: any fee payer. No party authority is required beyond a valid transaction.

Preconditions:

- State `Funded` and `now >= deliver_by`, or state `Delivered` and `now >= resolve_by`.
- Destination is the stored buyer destination.

Effects: checked-transfer exactly `principal` to the buyer destination and set state `Refunded`.

On `Paid` or `Refunded`, every one of these instructions returns the stable custom error `AlreadyComplete` and performs no transfer. The adapter maps that error plus an inspect result to the already-known terminal outcome.

## Custom errors

| Code | Name | When |
| --- | --- | --- |
| 6000 | `Unauthorized` | Wrong signer or wrong upgrade authority. |
| 6001 | `BadState` | Instruction does not apply to the current state. |
| 6002 | `Deadline` | Time precondition failed. |
| 6003 | `Amount` | For a positive principal, the amount debited from the source or credited to the destination differs from that principal. |
| 6004 | `Mint` | Mint or decimals disagree with config. |
| 6005 | `Destination` | Account is not the derived ATA, or mint/owner/token-program checks fail. |
| 6006 | `AlreadyComplete` | Terminal replay. |
| 6007 | `RoleCollision` | Two roles share a key. |
| 6008 | `ZeroPrincipal` | A new `fund` requests zero principal. Reject before transferring tokens. |
| 6009 | `BadTokenProgram` | Token program is not classic SPL Token. |
| 6010 | `BadDeployment` | Config, environment, or genesis binding failed. |
| 6011 | `DeliveryMissing` | Release without a stored delivery id. |
| 6012 | `DeadlineOrder` | The three deadlines are not strictly increasing. |

## Time and failure atomicity

Read `Clock.unix_timestamp`. Comparisons use the state-machine table. A failed token transfer fails the transaction, so the escrow account remains in its previous state. The adapter records the failed attempt and leaves the local projection unchanged. A frozen token account, a token-issuer restriction, or a halted cluster delays the transfer. Timeout eligibility assumes a functioning chain, a transferable token, and a caller willing to pay the network fee.

## Dust and closed destinations

Transfer exactly the stored principal, never `vault.amount`. Extra tokens in the vault remain there. Their presence does not stop `release` or `refund`.

The buyer or seller ATA may be closed after funding. The later transaction may contain one idempotent ATA create for that exact address, funded by the fee payer, without the beneficiary's signature. The program then sees the reopened account and still pays the stored destination. Tests cover a closed buyer ATA on refund and a closed seller ATA on release.

## Signer review policy

Before a buyer, seller, or verifier key signs, the adapter decodes the full transaction. The allowed top-level instructions are:

1. Zero or more compute-budget instructions that set the unit limit or unit price and carry no token accounts.
2. Zero or one associated-token idempotent create whose mint, owner, token program, and resulting address are the vault on `fund`, the seller ATA on `release`, or the buyer ATA on `refund`.
3. Exactly one `settle463-escrow` instruction, with the expected discriminator and the expected account list and arguments.

Reject every other program id. Reject a system-program transfer of SOL to any destination other than the rent required for the escrow account or the ATA create. Both funding signers must be shown the same decoded transaction. If the blockhash expires, decode the rebuilt transaction again and sign only after that review.

The fee payer signature is not a buyer, seller, or verifier signature unless that fee payer is actually that party.

## Adapter reconciliation

`FinalizedSuccess` applies to one action. Fetch its transaction by signature from the trusted RPC at finalized commitment and require transaction metadata with `meta.err == null`. For a submitted intent, match the locally stored signed transaction and its reviewed instruction list. For an independently submitted refund, identify the escrow instruction and attribute the transfer to its invocation of the classic SPL Token program. Check the expected action, arguments, signers, PDAs and accounts against the signed agreement. Read the program-owned escrow at finalized commitment with a context slot at least as recent as the transaction slot. Verify its immutable agreement IDs, parties, policy, mint, destinations, deadlines and principal.

| Action | Evidence in that finalized transaction | Required finalized escrow observation |
| --- | --- | --- |
| `fund` | Buyer ATA loses exactly the principal and the vault gains exactly the principal in the agreed mint. | Funding created the agreed escrow. A subsequent read may show `Funded`, `Delivered`, `Paid`, or `Refunded` with the same immutable terms. |
| `commit_delivery` | The seller commits the signed delivery ID. The transaction moves zero principal. A commit can omit token accounts. | `delivery_set == 1` and the stored ID equals that delivery. The state is `Delivered`, `Paid`, or `Refunded` after delivery. |
| `release` | The designated verifier authorizes the committed delivery ID and signed PASS verdict ID. The vault loses exactly the principal and the seller ATA gains it. | `Paid`, with both ID flags set and the matching delivery and verdict IDs. |
| `refund` | The vault loses exactly the principal and the buyer ATA gains it. | `Refunded` and `verdict_set == 0`. For a timeout before delivery, `delivery_set == 0`. For a timeout after delivery, `delivery_set == 1` and its ID matches the committed delivery. |

Measure transfer deltas in integer atomic units using the transaction's pre/post token balances for the expected account indices and mint. Vault dust can remain. A missing pre-balance counts as zero only for an ATA proven newly created and initialized empty in that transaction. Missing required evidence leaves the action unverified. A finalized account read can corroborate an earlier action after the order advances. Its balance is never a substitute for that transaction's deltas.

`Pending` covers processed or confirmed attempts. A finalized transaction with an execution error is `FinalizedFailure`. `Expired` requires an expired blockhash and evidence that the attempt did not land. Missing history that leaves execution uncertain is `Unknown`. Only verified `release` or `refund` success permits a receipt, after also verifying the linked funding and any delivery. A restart that discovers a different party's eligible refund reconciles that refund under its own signature. It must preserve the original intent's actual outcome.

Inspect is a decoded `getAccount` of the escrow PDA plus the vault token balance. It is not an instruction.

## Local validator asset

Phase D creates a fresh mint with six decimals on the classic token program. The adapter records the mint address from the mint account it just created. Tests fund the buyer ATA from that mint. SOL airdrops pay fees and rent only. Assertions compare token balances, not SOL balances, for principal.

## Devnet asset

Phase E checks, before any signature:

1. The Circle-published Solana devnet USDC address. The planning reference is `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`. If Circle's list differs on the day of the run, use the list and record the URL and the value. Abort the run if the list cannot be fetched.
2. The mint account exists on the trusted devnet RPC, decimals are 6, and the owner is the classic token program.
3. The RPC genesis hash, the deployed program id, and the config PDA match the environment string that will be signed.
4. The upgrade authority is the test-only deployer key recorded in the evidence. Test SOL and test USDC come from public faucets or an already funded fixture. They are not live money.

The local mock mint and the devnet USDC mint are different deployments. A message signed for one environment is invalid on the other.
