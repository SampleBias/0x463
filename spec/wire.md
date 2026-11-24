# Wire contract

The wire version is the literal string `0.1`. Crate versions may be `0.1.0`. Those two strings are different and both stay.

## Encodings

| Value | Encoding |
| --- | --- |
| 32-byte agent keys, hashes, nonces, message ids | Lowercase hex, exactly 64 characters. |
| 64-byte Ed25519 signatures | Lowercase hex, exactly 128 characters. |
| Solana account addresses | Canonical base58, Bitcoin alphabet, exactly 32 decoded bytes. |
| Amounts, byte counts, timestamps inside signed JSON | Canonical unsigned decimal strings. |

Decimal strings are `"0"` or a digit `1` through `9` followed by digits. Reject `+`, `-`, `.`, exponents, spaces, and leading zeros. Amounts and byte counts fit in `u64`. Timestamps fit in nonnegative `i64` seconds.

Reject uppercase hex. Reject a base58 string that re-encodes to a different string. An agent key and its Solana address are the same 32 bytes.

## JSON subset

Protocol documents use objects, arrays, strings, and booleans. Reject JSON numbers, JSON null, invalid UTF-8, truncated input, and trailing data after one top-level value.

Reject duplicate object keys at every depth before map deserialization. A parser that keeps the last duplicate is incorrect.

Reject unknown fields at every object that this contract defines.

Optional fields are omitted. They are never null.

String limits unless a field below is tighter: 4096 UTF-8 bytes. Arrays that this contract defines have a maximum length stated on the field.

## Canonicalization and signatures

Signing and verification use this order:

1. Start from the JSON object.
2. Remove the outer `signature` field.
3. Strictly validate the remaining object, including the kind body and the caller's expected environment.
4. Canonicalize the validated object with RFC 8785 JSON Canonicalization Scheme (JCS).
5. Build the preimage.
6. Sign or verify that preimage with Ed25519.

The preimage is the ASCII bytes of `0x463/signed/0.1`, then one `0x00` byte, then the canonical UTF-8 bytes.

Sign those preimage bytes with Ed25519 (RFC 8032). Do not hash them again before calling the signer. `ed25519-dalek` applies the RFC 8032 SHA-512 internally. Verify with the strict verifier so a non-canonical signature is rejected.

The message id is the lowercase hex SHA-256 of that same preimage. Message ids are not fields inside the hashed object. References to earlier messages are ids, policy digests, raw artifact digests, or ledger locators.

JCS requirements that tests must pin:

- No insignificant whitespace.
- Object keys ordered by UTF-16 code units, not by raw UTF-8 bytes.
- Apply the same string serialization to property names and values. Escape U+0000 through U+001F using `\b`, `\t`, `\n`, `\f`, `\r`, or lowercase `\u00hh` as appropriate. Escape double quote and backslash. Emit every other Unicode scalar, including slash, U+2028 and U+2029, directly as UTF-8. Reject lone surrogates. [RFC 8785 §3.2.2.2](https://www.rfc-editor.org/rfc/rfc8785.html#section-3.2.2.2)
- The JSON input `{"\u2028":"\u2029"}` and the same object with literal separators both canonicalize to hex `7b22e280a8223a22e280a9227d`. [Regression vector](../plan/golden/jcs-line-separators.json)
- The UTF-16 order test from the golden file: the canonical UTF-8 hex of `{"😀":"grin","\uE000":"pua"}` with emoji first is `7b22f09f9880223a226772696e222c22ee8080223a22707561227d`. A sorter that compares UTF-8 bytes reverses the keys and must fail this test.

Valid v0.1 messages do not contain JSON numbers, so the number-serialization section of JCS is not on the acceptance path. Reject numbers during strict validation. Keep a test that a numeric token is rejected.

## Identifiers

### Order id

Input object, exactly three fields:

```json
{"buyer":"<buyer signing key hex>","environment":"<environment>","nonce":"<32-byte hex>"}
```

The preimage is ASCII `0x463/order/0.1`, one `0x00` byte, then the JCS bytes of that object. The order id is lowercase hex SHA-256 of the preimage. The nonce is a cryptographically random 32-byte value except inside deterministic fixtures. The order id does not change when the quote changes. One request cannot fund twice.

### Policy hash

The preimage is ASCII `0x463/policy/0.1`, one `0x00` byte, then the JCS bytes of the acceptance-policy object defined below. The policy hash is lowercase hex SHA-256 of that preimage. The hashed object is the acceptance policy alone, not the surrounding quote.

### Artifact digest

SHA-256 of the raw artifact bytes. Lowercase hex.

## Envelope

Every signed message has exactly these outer fields:

| Field | Rule |
| --- | --- |
| `protocol` | Literal `0x463`. |
| `version` | Literal `0.1`. |
| `kind` | One of the closed kinds below. |
| `environment` | A pinned deployment identifier. |
| `issuer` | Lowercase hex public key that must verify the signature. |
| `body` | Kind object. |
| `signature` | Lowercase hex Ed25519 signature over the preimage. Present on the wire. Absent from the signed preimage. |

Closed kinds: `service_offer`, `request`, `quote`, `accept`, `delivery`, `verdict`, `receipt`, `observation`.

The issuer role must match the kind:

| Kind | Issuer |
| --- | --- |
| `service_offer` | Seller |
| `request` | Buyer |
| `quote` | Seller |
| `accept` | Buyer |
| `delivery` | Seller |
| `verdict` | Verifier |
| `receipt` | Receipt issuer |
| `observation` | Receipt issuer |

## Common objects

### Asset

```json
{"decimals":"6","id":"<id>","kind":"sim"}
```

or

```json
{"decimals":"6","id":"<mint base58>","kind":"spl-token"}
```

`id` is `test-six-decimal` when `kind` is `sim`. `id` is a mint address when `kind` is `spl-token`. Decimals in v0.1 quotes are `"6"`. The chain still checks the mint account.

### Acceptance policy

```json
{
  "expected_digest": "<hex32>",
  "expected_length": "<decimal u64>",
  "limits": {
    "max_bytes": "1048576",
    "max_field_name_bytes": "64",
    "max_fields": "64",
    "max_string_bytes": "4096"
  },
  "profile": "sha256-json-object-v1",
  "schema": {
    "<field-name>": "string"
  }
}
```

`schema` values are `string`, `bool`, or `u64_decimal`. One to 64 fields. `expected_length` is less than or equal to `limits.max_bytes`. `limits.max_bytes` is less than or equal to `"1048576"`. The reference schema is `job` → `string`, `result` → `string`, `units` → `u64_decimal`.

Reject a policy that asks for a higher limit, a nested schema, an array schema, or a profile other than `sha256-json-object-v1` before funding.

## Kind bodies

### `service_offer`

| Field | Rule |
| --- | --- |
| `service_id` | `^[a-z][a-z0-9-]{0,63}$` |
| `descriptor_version` | Literal `"1"` |
| `input_profile.content_type` | `application/json` |
| `input_profile.max_bytes` | Decimal, `1` through `1048576` |
| `input_profile.schema_profile` | `sha256-json-object-v1` |
| `output_profile.content_type` | `application/json` |
| `output_profile.schema_profile` | `sha256-json-object-v1` |
| `verification_profiles` | One to eight unique entries. v0.1 verifiers implement `sha256-json-object-v1`. |
| `environments` | One to eight unique environment strings. |
| `expires_at` | Nonnegative decimal timestamp. |

Discovery returns this signed object. The demo configures trusted publisher keys and refuses offers from other issuers.

### `request`

| Field | Rule |
| --- | --- |
| `order_id` | Must equal the derived order id. |
| `nonce` | 32-byte hex used in that derivation. |
| `service_id` | Matches the offer. |
| `offer_id` | Message id of the `service_offer`. |
| `buyer` | Equals the envelope issuer. |
| `seller` | Intended seller. Equals the offer issuer. |
| `artifact_constraints.verification_profile` | `sha256-json-object-v1` |
| `artifact_constraints.max_bytes` | Decimal, within the offer maximum and within 1 MiB. |
| `allowed_verifiers` | One to eight unique keys. Must not contain the buyer or the seller. |
| `asset` | Asset object. |
| `max_principal` | Decimal `u64`, greater than zero. |
| `expires_at` | Nonnegative decimal timestamp. |

A request is an invitation to quote.

### `quote`

| Field | Rule |
| --- | --- |
| `request_id` | Message id of the request. |
| `order_id` | Same order id. |
| `offer_id` | Same offer id. |
| `buyer` | Request buyer. |
| `seller` | Equals the envelope issuer and the request seller. |
| `verifier` | One key from `allowed_verifiers`. Distinct from buyer, seller, and receipt issuer. |
| `receipt_issuer` | Coordinator receipt key. Distinct from buyer, seller, and verifier. |
| `service_id` | Same service id. |
| `artifact.digest` | Expected raw SHA-256. |
| `artifact.length` | Decimal byte length. |
| `artifact.verification_profile` | `sha256-json-object-v1` |
| `artifact.max_bytes` | Decimal within the request constraint. |
| `acceptance_policy` | The object hashed into `policy_hash`. Its digest, length, profile, and schema match `artifact`. |
| `policy_hash` | Derived policy hash. |
| `amount` | Decimal `u64`. Greater than zero. Less than or equal to `max_principal`. |
| `asset` | Asset object. Decimals `"6"`. |
| `settlement` | Settlement object below. |
| `deadlines.fund_by` | Decimal timestamp. |
| `deadlines.deliver_by` | Decimal timestamp. |
| `deadlines.resolve_by` | Decimal timestamp. |
| `expires_at` | Decimal timestamp. `fund_by` is less than or equal to this value and less than or equal to the request `expires_at`. |
| `fees.protocol_atomic` | Literal `"0"`. |
| `fees.network_fee_payer` | `buyer`, `seller`, `verifier`, or `external`. |
| `artifact_store.store_id` | `^[a-z][a-z0-9-]{0,63}$` |
| `artifact_store.read_access` | Literal `order-parties` |
| `artifact_store.retention_seconds` | Decimal `u64`. |

Deadlines satisfy `fund_by < deliver_by < resolve_by`.

Simulator settlement object:

```json
{
  "asset_id": "test-six-decimal",
  "buyer_destination": "sim:<buyer hex>",
  "profile": "sim-ledger-v1",
  "seller_destination": "sim:<seller hex>"
}
```

Solana settlement object:

| Field | Rule |
| --- | --- |
| `profile` | `solana-spl-token-v1` |
| `mint` | Canonical base58. Equals `asset.id`. |
| `token_program` | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` |
| `buyer_wallet` | Base58 of the buyer key. |
| `seller_wallet` | Base58 of the seller key. |
| `buyer_token_account` | Canonical ATA of the buyer wallet for the mint and the classic token program. |
| `seller_token_account` | Canonical ATA of the seller wallet. |
| `escrow_pda` | PDA derived in [the escrow PDA rules](settlement-solana.md#escrow-pda) from the buyer key and the request nonce. |
| `vault_token_account` | Canonical ATA of `escrow_pda`. |
| `config` | Config PDA. |
| `program_id` | Deployed program id. Equals the program id inside `environment`. |

Both funding signers recompute the PDA and the ATAs and compare them to this object.

### `accept`

| Field | Rule |
| --- | --- |
| `request_id` | Request message id. |
| `quote_id` | Selected quote message id. |
| `order_id` | Order id. |
| `amount` | Equals the quote amount. |
| `asset` | JCS-equal to the quote asset. |
| `environment` | Equals the envelope environment and the pinned deployment. |

Persist this acceptance under the buyer and nonce before any wallet signs a funding transaction. One accepted quote per request, enforced by a transactional uniqueness constraint. A second economic attempt needs a new request nonce.

### `delivery`

| Field | Rule |
| --- | --- |
| `order_id` | Order id. |
| `quote_id` | Accepted quote id. |
| `policy_hash` | Quote policy hash. |
| `digest` | SHA-256 of the uploaded raw bytes. |
| `length` | Decimal length of those bytes. |
| `content_id` | 32-byte lowercase hex assigned by the store. Not a URL and not a filesystem path. |

The chain stores this message id. One on-chain delivery per order.

### `verdict`

| Field | Rule |
| --- | --- |
| `order_id` | Order id. |
| `quote_id` | Quote id. |
| `delivery_id` | Delivery message id that was committed. |
| `policy_hash` | Policy hash. |
| `algorithm` | `sha256-json-object-v1` |
| `algorithm_version` | `"1"` |
| `decision` | `PASS`, `FAIL`, or `INDETERMINATE` |
| `reason_codes` | Closed reason codes from the [verifier profile](verifier-sha256-json-object-v1.md#artifact-and-verifier-profile). |
| `observed_digest` | Required when the bytes were obtained. Equals the raw SHA-256. |
| `observed_length` | Required when the bytes were obtained. |

`PASS` requires both observations, `reason_codes` equal to `["MATCH"]`, digest and length equal to the policy, and a passing schema check. `FAIL` and `INDETERMINATE` never authorize release.

### `receipt`

Required on every receipt:

| Field | Rule |
| --- | --- |
| `order_id` | Order id. |
| `outcome` | `paid`, `refunded_timeout_before_delivery`, or `refunded_timeout_after_delivery` |
| `principal` | Funded amount, decimal. |
| `asset` | JCS-equal to the quote asset. |
| `buyer`, `seller`, `verifier` | Role keys. |
| `receipt_issuer` | Equals the envelope issuer and the quote's receipt issuer. |
| `request_id`, `quote_id`, `acceptance_id` | Message ids. |
| `environment` | Envelope environment. |
| `destination` | Account that received the principal. |
| `ledger_kind` | `sim` or `solana` |
| `terminal_locator` | Simulator locator or Solana transaction signature. |
| `finality` | `simulated-final` or `finalized` |

Conditional fields follow the [receipt outcome table](receipts.md#receipt-outcomes). `delivery_id` and `verdict_id` are omitted when that outcome does not include them. When a FAIL verdict is included on a timeout after delivery, `verdict_id` is that message id and the verdict decision is `FAIL`.

Simulator `terminal_locator` format: `sim:<order_id>:<decimal event seq>`.

### `observation`

| Field | Rule |
| --- | --- |
| `order_id` | Order id. |
| `event_seq` | Decimal `u64`, starts at `1`. |
| `event_digest` | Hex SHA-256 defined in [the audit hash rules](persistence.md#audit-hash). |
| `event_type` | Closed event name from the API contract. |
| `refs` | Zero to sixteen strings. Each string is a message id or a ledger locator. |

## Linkage

A component that accepts a message checks the signature, the issuer role, and the links. Cryptographic validity alone is not enough.

- Request issuer, `body.buyer`, quote buyer, and accept issuer are one key.
- Offer issuer, request seller, quote issuer, quote seller, and delivery issuer are one key.
- Quote verifier and verdict issuer are one key.
- Quote receipt issuer, receipt issuer, and observation issuer are one key.
- Buyer, seller, verifier, and receipt issuer are pairwise distinct.
- `request.order_id` matches the derivation from environment, buyer, and nonce.
- Quote request id, order id, offer id, and service id match the request and the offer.
- Accept references that request, that quote, and that order, and repeats the amount, asset, and environment.
- Delivery and verdict reference the same order, quote, and policy hash.
- Verdict `delivery_id` is the delivery message id.
- Recomputed policy hash matches `policy_hash`.
- Every envelope in the order carries the same environment.
- Quote amount is less than or equal to the request maximum.
- Quote verifier is inside `allowed_verifiers`.
- Offer is unexpired at the time the request is signed, using the clock of the component that is signing.

## Buyer and seller policies

Buyer policy, checked before the buyer signs a request or an acceptance:

| Field | Demo value |
| --- | --- |
| `max_principal` | `2500000` |
| `allowed_verifiers` | The demo verifier key |
| `allowed_services` | `sample-json-report` |
| `allowed_assets` | The demo asset id for that backend |
| `allowed_environments` | The process environment |
| `max_deliver_window_seconds` | `86400` |
| `max_resolve_window_seconds` | `86400` |

The SDK refuses to sign when the quote amount exceeds the policy or the request, the verifier, service, asset, or environment is outside the policy, a deadline window exceeds the policy, the protocol fee is not zero, or the roles collide.

Seller policy, checked before the seller signs a quote or a funding transaction: minimum amount, allowed verifiers, allowed environments, and exact equality between the funding transaction's amount, mint, destinations, and the seller's own quote. The seller refuses to sign a release.

Verifier policy: sign a PASS verdict only after the local checks pass, and sign a release transaction only when it carries that verdict id, the stored delivery id, the quoted seller destination, and the exact principal.

The supplied vector gives canonical bytes, signing preimages, signatures and derived IDs for the reference offer. [Positive signing vector](../fixtures/golden/positive-signing-vector.json)
