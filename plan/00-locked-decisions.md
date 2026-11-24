# Locked decisions

These decisions are closed by the v0.1.0 master specification and by this plan. Later phases implement them. They are not open design questions.

## Product bounds

| Item | v0.1 value |
| --- | --- |
| Parties per order | One buyer, one seller, one verifier |
| Artifact | One precommitted byte string |
| Price | One fixed amount, one asset |
| Protocol fee | Zero. The decimal string is `"0"`. |
| Negotiation | Seller publishes a signed offer. Buyer requests terms with a price ceiling. Seller quotes a fixed price. Buyer accepts that quote or requests another quote. |
| Quote | Creates no spend obligation. An amended quote is a new signed quote. |
| Coordinator | Optional for discovery and relay. Required keys for money stay with the agents. |
| Direct exchange | The same signed bytes must also move between configured agent endpoints. |
| Chains | Simulator and Solana. The domain model does not import Solana types. |
| Asset on the simulator | `test-six-decimal`, kind `sim`, six decimals. It is a test asset. |
| Asset on local validator | A generated six-decimal mint owned by the classic SPL Token program. Name it as a test asset in every log and receipt. |
| Asset on devnet | Circle's published devnet USDC mint, re-checked at execution time against the Circle address list. As of the Circle page reviewed for this plan (11 November 2026) that mint is `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`. |
| Forbidden mint selection | A ticker string, a mainnet USDC mint, or any mint whose on-chain decimals or token-program owner do not match the deployment config. The mainnet USDC mint `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` is out of scope. |

Demo price is `2000000` atomic units (2.00 units at six decimals). The demo buyer's maximum principal is `2500000` atomic units.

## Roles and trust

| Role | Holds | Does not hold |
| --- | --- | --- |
| Buyer | Spending key. Policy for maximum principal, verifiers, services, assets, environments, and deadline bounds. | Seller or verifier key. |
| Seller | Signing key. Quotes terms, signs funding, uploads the artifact, commits one delivery. | The right to approve its own payout. |
| Verifier | A distinct key accepted by both parties before funding. | Buyer, seller, and coordinator keys. |
| Coordinator | Receipt-signing key and a database of messages, blobs, and audit rows. | Any party spending key. Any authority to change an escrow outcome. |

The verifier is trusted to report honestly and to be available.

- A dishonest verifier can authorize release of a bad artifact by signing a PASS and the release transaction.
- An unavailable verifier causes valid work to reach `resolve_by` and be refunded. Silence never pays the seller.

State both consequences in `spec/threat-model.md` and in the README.

Acceptance means the agreed verifier observed bytes that match the committed rules. In the reference implementation the buyer can download those same bytes. A verdict does not prove legal rights, secrecy, semantic quality, or real-world completion. Profile `sha256-json-object-v1` fits artifacts with a known digest or reproducible expected output.

Every agent validates the transaction it is asked to sign, including every instruction and every account destination. An independent participant who has the signed order bundle can inspect the escrow and invoke an eligible refund through the SDK while the coordinator is offline.

## Clock

Authoritative time is the simulator's injected clock or the Solana `Clock` sysvar `unix_timestamp` at instruction execution. A wall clock may schedule retries. A wall clock cannot accept or reject a transfer.

Deadlines are absolute Unix seconds. They satisfy `fund_by < deliver_by < resolve_by`. `fund_by` is less than or equal to the request expiry and less than or equal to the quote expiry.

At the exact deadline second, the expiring action is invalid and the timeout path is valid.

| Boundary | At `now == deadline` |
| --- | --- |
| `fund_by` | `fund` is invalid. No financial receipt is created for an order that never funded. |
| `deliver_by` | `commit_delivery` is invalid. `refund` from `Funded` is valid. |
| `resolve_by` | `release` is invalid. `refund` from `Delivered` is valid. |

One tick before a deadline, the still-open action is valid and the timeout path is invalid. One tick after, the timeout path stays valid and the expired action stays invalid.

## State machine

| Current | Command | Authority and preconditions | Result |
| --- | --- | --- | --- |
| Unfunded | `fund` | Buyer and seller. Terms valid. `now < fund_by`. Exact principal. | Funded. Principal held. |
| Funded | `commit_delivery` | Seller. `now < deliver_by`. First commitment. | Delivered. Delivery id stored. |
| Delivered | `release` | Designated verifier. PASS report. `now < resolve_by`. Stored delivery id present. | Paid. Exact principal to the seller destination. Verdict id stored. |
| Funded | `refund` | Any caller. `now >= deliver_by`. | Refunded. Exact principal to the buyer destination. |
| Delivered | `refund` | Any caller. `now >= resolve_by`. | Refunded. Exact principal to the buyer destination. |

`Paid` and `Refunded` are terminal. A replay returns the known result or the stable already-completed error and moves zero principal. A terminal order cannot be reopened.

There is no early refund on FAIL. A FAIL report is evidence. The refund deadline stays in force. `INDETERMINATE` is per attempt and retryable. The first definitive PASS or FAIL for a committed delivery is persisted. Later verdict calls return that same signed decision.

A previously signed PASS submitted at or after `resolve_by` cannot pay.

Verification of bytes happens off-chain. On-chain release enforcement is the designated verifier's signature over the release transaction. The verifier service signs that transaction only for the exact PASS report.

## Money invariant

For every order, after any successful or attempted operation:

```text
funded principal = principal still held + principal paid + principal refunded
```

Use checked integer arithmetic. The maximum seller payout is the funded amount. Refund only to the committed buyer destination. SOL transaction fees, account rent, and unsolicited tokens that land in the vault are outside principal. No operator instruction can withdraw or redirect principal.

Unsolicited vault deposits stay in the vault. Terminalization transfers exactly the funded principal. A nonempty vault does not block terminalization.

A payment submission, an observed unfinalized transfer, and a verifier PASS are three different states. None of them is finalized payment. A receipt is emitted only after finalized terminal success and a matching escrow observation.

Reusing a buyer nonce on one deployment creates one `order_id`. At most one escrow deposit exists for that `order_id`.

## Identity binding on Solana

For v0.1 the buyer, seller, and verifier agent public keys are the Solana signer public keys. The 32-byte Ed25519 key and the canonical base58 account address round-trip to the same bytes. Roles are distinct: buyer, seller, verifier, and receipt issuer are four different keys.

The network fee payer never gains authority over principal by paying fees or recreating an associated token account.

## Environment

```text
sim:<deployment-id>
solana:<genesis-base58>:<program-base58>:<config-base58>
```

A simulated deployment id is 1 to 64 characters, matching `^[a-z0-9]([a-z0-9-]{0,62}[a-z0-9])?$`. Each live simulator process uses its own deployment id. Deterministic fixtures may use `sim:fixture`.

Reject every message whose `version` is not `0.1` or whose `environment` is not the caller's pinned environment. An unrelated application or deployment cannot replay a signed message.

The Solana environment's genesis hash comes from the caller's trusted RPC. The receipt is not the source of the RPC URL, the genesis hash, or the program id used as a trust anchor.

## Toolchain pin procedure

Phase A pins the toolchain after a spike. The planning host on 24 November 2026 had `rustc 1.97.0`, `cargo 1.97.0`, `solana-cli 3.1.8`, and `anchor-cli 0.32.1`. That combination is a starting observation, not the pin. Anchor 0.32.x recommends Solana 2.3.0. Anchor 1.2.2 (28 October 2026) is the latest 1.x release observed while planning. Anchor 2.0.0 release candidates are out of scope for v0.1.

Spike rule:

1. Choose one published Anchor CLI and matching `anchor-lang` / `anchor-spl` versions.
2. Choose the Solana CLI those Anchor release notes support, and install it if the host CLI does not match.
3. Build a one-instruction program, deploy it to `solana-test-validator` from that CLI, and call the instruction.
4. Record host `rustc`, program platform-tools Rust, Solana CLI, Anchor CLI, and `cargo-build-sbf` in `spec/toolchain.md` and in `rust-toolchain.toml` / `Anchor.toml`.
5. If the installed validator rejects the binary because of an SBPF version, rebuild with the architecture that validator accepts and record the flag. Devnet may require a newer SBPF than the local validator. Phase E checks that separately.

Commit `Cargo.lock`. Off-chain code uses the pinned stable Rust. The program may use the platform-tools compiler. Document that difference.

Preferred off-chain crates, versions selected at the spike so they compile together: Tokio, Axum, Serde, `ed25519-dalek`, `sha2`, SQLx with SQLite, `rustls` for remote traffic, Clap for the CLI. Verify signatures with `ed25519-dalek`'s strict verifier.

## Persistence rules that affect money

- Persist the financial intent, including the locally signed transaction bytes and the locally computed signature, before broadcast.
- A lost RPC response is reconciled with `getTransaction` and escrow account data. A signature string by itself is not finality.
- After blockhash expiry, rebuild only when reconciliation establishes non-execution and the action remains eligible. The signers revalidate the rebuilt transaction before signing.
- Bounded retries with backoff. A durable queue. A visible pending or unknown state.
- If archival history is unavailable, report the gap. Do not manufacture a receipt.
- At most one economic outcome per order. At-least-once delivery stays idempotent.
- HTTP idempotency keys are scoped to caller and operation. The same key and the same body return the original result. The same key and a different body return a conflict. A new HTTP key cannot authorize a second economic action.
- Store full-range `u64` values as canonical decimal text. SQLite integers cannot store every `u64`.
- Persist the original signed bytes.
- A write and its outbox row commit in one database transaction.

## Artifact and verifier profile

Profile name: `sha256-json-object-v1`.

- Hash the raw bytes with SHA-256. Do not reserialize them before hashing.
- The artifact is one flat UTF-8 JSON object. Leading or trailing bytes, including a newline, make the artifact different.
- Require every field in the committed schema. Reject extra fields, duplicate keys, arrays, nested objects, JSON numbers, and JSON null.
- Schema value types are `string`, `bool`, and `u64_decimal`.
- Hard maximum artifact size is 1 MiB (1048576 bytes). Maximum fields is 64.
- Maximum field-name length is 64 bytes. Maximum string value length is 4096 bytes.
- Reject an unsupported schema feature before funding.
- Execute no submitted code, shell command, model, remote schema, or arbitrary URL.
- Built-in versioned Rust is the implementation. Algorithm version on a verdict is `"1"`.

Reason codes are a closed list, sorted lexicographically in the signed verdict, unique, length 1 to 8:

| Code | When |
| --- | --- |
| `MATCH` | PASS only, and it is the only code. |
| `DIGEST_MISMATCH` | Raw SHA-256 differs from the policy. |
| `SIZE_MISMATCH` | Byte length differs from the policy. |
| `SCHEMA_MISMATCH` | Extra field, missing field, or unsupported shape. |
| `FIELD_TYPE` | A present field has the wrong JSON type. |
| `MALFORMED_JSON` | The bytes are not one JSON object. |
| `DUPLICATE_FIELD` | A duplicate object key appears at any depth. |
| `OVERSIZED` | Length exceeds the policy maximum or 1 MiB. |
| `TOO_MANY_FIELDS` | More than 64 fields. |
| `FIELD_LENGTH` | A name or string exceeds its limit. |
| `UNSUPPORTED_PROFILE` | The quote names a profile this verifier does not run. |
| `UNAVAILABLE` | Retrieval, storage, or RPC needed for the bytes failed. Decision is `INDETERMINATE`. |
| `DEADLINE_EXPIRED` | The verifier read authoritative ledger time at or after `resolve_by`. Decision is `INDETERMINATE`, not FAIL. |

Run the schema check even when the digest mismatches, and list every applicable code. A digest match must not hide a schema failure. A one-byte mutation must not produce PASS.

`DEADLINE_EXPIRED` is not a sticky FAIL. A local wall clock must not burn the one definitive verdict.

Transport limits and content checks are separate. Store-relative content ids never become caller-chosen filesystem paths or network hosts. Reads are authorized by order membership, not by knowledge of a digest. Writes are immutable. Re-uploading the same bytes is idempotent. Uploading different bytes creates a different content id. Only one delivery can be committed.

A relay outage can block delivery and can end in a timeout refund. Document that.

Reads of the fixture remain available at least until `resolve_by` plus the quoted retention window.

## Receipt outcomes

| Outcome | Delivery in the receipt | Verdict in the receipt | Destination |
| --- | --- | --- | --- |
| `paid` | Required | Required, decision PASS | Seller |
| `refunded_timeout_before_delivery` | Absent | Absent | Buyer |
| `refunded_timeout_after_delivery` | Required | Absent, or a FAIL verdict | Buyer |

A refund receipt does not contain an invented verifier signature. A PASS bundled together with a refund outcome is inconsistent and fails offline verification.

## Coordinator observations

The closed protocol kinds are:

`service_offer`, `request`, `quote`, `accept`, `delivery`, `verdict`, `receipt`, `observation`.

`observation` is signed by the receipt issuer. It authenticates an audit event. It does not move money. A local hash chain alone is not protection against replacement of the whole log. Chain evidence is a separate object from an observation signature.

## Demo clock and deadlines

Deterministic conformance orders may use:

| Field | Value |
| --- | --- |
| `T0` and simulator start | `1700000000` |
| Offer `expires_at` | `1700007200` |
| Request `expires_at` | `1700003600` |
| Quote `expires_at` | `1700003600` |
| `fund_by` | `1700003600` |
| `deliver_by` | `1700007200` |
| `resolve_by` | `1700010800` |
| Retention seconds | `86400` |
| Service id | `sample-json-report` |
| Network fee payer recorded on the quote | `buyer` |

Live CLI demos still use a fresh `sim:<deployment-id>` per process. They may use these same relative offsets from their own `T0`.

## Negative artifact bytes

Hash the raw bytes. Use these values in tests.

| Case | Bytes | Length | SHA-256 |
| --- | --- | --- | --- |
| Reference | `{"job":"sample-001","result":"complete","units":"3"}` | 52 | `37ba5c2e570e4e256af7049d0d9c27f70b50f5de4210101d7c0bd2b497db7587` |
| One-byte value change | `{"job":"sample-001","result":"complete","units":"4"}` | 52 | `d0d1d08718dc53380445736580bb4bcebcc56204656063add46afa9504a3670f` |
| Extra field, schema-invalid, own order | `{"job":"sample-001","result":"complete","units":"3","extra":"x"}` | 64 | `a6c4530d161e5de02e365783fe7dbb96d93e6334ba78f0b52257e0b7fa75635a` |
| JSON number where a decimal string is required | `{"job":"sample-001","result":"complete","units":3}` | 50 | `58cda464d29ad6b71984742464aae5f6cfcd0c3855758c0d93ec9ce8f2305e74` |
| Duplicate key | `{"job":"sample-001","job":"other","result":"complete","units":"3"}` | 66 | `8b67e670e01404b64c72e1814c32d549a23fc84c42271a8d589ba94b3e81c6dc` |
| Malformed | `{` | 1 | `021fb596db81e6d02bf3d2586ee3981fe519f275c0ac9ca76bbcf2ebb4097d96` |

For the extra-field order, the policy `expected_digest` is that row's SHA-256 and `expected_length` is `64`. The schema still requires only `job`, `result`, and `units`. The passing verdict condition is unreachable. The reason list contains `SCHEMA_MISMATCH` and does not contain `DIGEST_MISMATCH`.

## Security claims that are intentionally limited

- Offline verification reports `evidence_signed` when signatures, identities, linkage, policy, and outcome structure agree. It does not prove ledger finality.
- Online verification reports `ledger_verified` only after the trusted RPC and the pinned deployment match the funding instruction, any seller commitment, the terminal instruction, and the token movement at finalized commitment.
- The coordinator database is not a source of payment authority.
- Fixture keys in `plan/golden/` and `fixtures/` are test keys. Mark them as fixtures in filenames and comments. Keep them out of production configuration examples except as obvious placeholders.
