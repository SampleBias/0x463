# Artifact verification

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

A relay outage can block delivery and can end in a timeout refund.

Reads of the fixture remain available at least until `resolve_by` plus the quoted retention window.

## Negative artifact bytes

Hash the raw bytes. Use these values in tests.

| Case | Bytes | Length | SHA-256 |
| --- | --- | --- | --- |
| [Reference](../fixtures/artifacts/reference.json) | `{"job":"sample-001","result":"complete","units":"3"}` | 52 | `37ba5c2e570e4e256af7049d0d9c27f70b50f5de4210101d7c0bd2b497db7587` |
| [One-byte value change](../fixtures/artifacts/one-byte-change.json) | `{"job":"sample-001","result":"complete","units":"4"}` | 52 | `d0d1d08718dc53380445736580bb4bcebcc56204656063add46afa9504a3670f` |
| [Extra field, schema-invalid, own order](../fixtures/artifacts/extra-field.json) | `{"job":"sample-001","result":"complete","units":"3","extra":"x"}` | 64 | `a6c4530d161e5de02e365783fe7dbb96d93e6334ba78f0b52257e0b7fa75635a` |
| [JSON number where a decimal string is required](../fixtures/artifacts/number.json) | `{"job":"sample-001","result":"complete","units":3}` | 50 | `58cda464d29ad6b71984742464aae5f6cfcd0c3855758c0d93ec9ce8f2305e74` |
| [Duplicate key](../fixtures/artifacts/duplicate-key.json) | `{"job":"sample-001","job":"other","result":"complete","units":"3"}` | 66 | `8b67e670e01404b64c72e1814c32d549a23fc84c42271a8d589ba94b3e81c6dc` |
| [Malformed](../fixtures/artifacts/malformed.json) | `{` | 1 | `021fb596db81e6d02bf3d2586ee3981fe519f275c0ac9ca76bbcf2ebb4097d96` |

For the extra-field order, the policy `expected_digest` is that row's SHA-256 and `expected_length` is `64`. The schema still requires only `job`, `result`, and `units`. The passing verdict condition is unreachable. The reason list contains `SCHEMA_MISMATCH` and does not contain `DIGEST_MISMATCH`.
