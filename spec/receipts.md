# Receipts

## Receipt outcomes

| Outcome | Delivery in the receipt | Verdict in the receipt | Destination |
| --- | --- | --- | --- |
| `paid` | Required | Required, decision PASS | Seller |
| `refunded_timeout_before_delivery` | Absent | Absent | Buyer |
| `refunded_timeout_after_delivery` | Required | Absent, or a FAIL verdict | Buyer |

A refund receipt does not contain an invented verifier signature. A PASS bundled together with a refund outcome is inconsistent and fails offline verification.

## Receipt bundle

```json
{
  "acceptance": {},
  "bundle_version": "0.1",
  "delivery": {},
  "environment": "sim:<id>",
  "ledger": {},
  "observations": [],
  "order_id": "<hex>",
  "quote": {},
  "receipt": {},
  "request": {},
  "verdict": {}
}
```

`delivery` and `verdict` are omitted when the outcome does not include them. The bundle excludes artifact bytes, private keys, challenge nonces, and temporary URLs.

Simulator `ledger`:

```json
{
  "asset_id": "test-six-decimal",
  "clock": "1700010800",
  "decimals": "6",
  "deployment_id": "<id>",
  "destination": "sim:<hex>",
  "finality": "simulated-final",
  "ledger_kind": "sim",
  "principal": "2000000",
  "terminal_locator": "sim:<order_id>:<seq>"
}
```

Solana `ledger` includes `ledger_kind` `solana`, genesis base58, program id, config, order PDA, mint, atomic amount, beneficiary, vault, funding signature, optional delivery signature, terminal signature, terminal instruction name (`release` or `refund`), finalized slot as a decimal string, and `evidence_digest`. The digest is SHA-256 of the JCS ledger object with `evidence_digest` removed.

The JSON field names are `genesis`, `program_id`, `config`, `order_pda`, `mint`, `amount`, `beneficiary`, `vault`, `funding_signature`, `delivery_signature`, `terminal_signature`, `terminal_instruction`, and `finalized_slot`. The amount is an unsigned decimal string in mint atomic units. The receipt's `terminal_locator` is the terminal transaction signature. The verifier reads the RPC endpoint from the caller's `--rpc` argument and ignores a top-level `rpc_url` hint. A finalized transaction may omit `blockTime`. The escrow program enforces deadlines using the Clock sysvar during execution.

## Verification modes

Offline checks:

- Each included message verifies under the strict signature rules.
- The receipt issuer matches the trust input.
- The order id matches the trust input and the derived id.
- Linkage, policy hash, amount, asset, and outcome rules hold.
- A timeout before delivery has no delivery and no verdict.
- A timeout after delivery has a delivery and no PASS verdict.
- The result label is `evidence_signed`.

Online checks add the caller's RPC and pinned deployment:

- `getGenesisHash` equals the trusted genesis.
- The trusted program id and config match the environment.
- The funding transaction is finalized, succeeded, and names the agreement ids and the exact principal.
- If a delivery exists, the commit transaction is finalized and stores that delivery id.
- The terminal transaction is finalized, succeeded, and moves the exact principal from the vault to the receipt destination.
- Apply the same per-action transaction and escrow checks as the reconciler. [Action evidence](settlement-solana.md#adapter-reconciliation)
- The result label is `ledger_verified`.

| Result | Exit code |
| --- | --- |
| `evidence_signed` or `ledger_verified` | 0 |
| Usage or missing trust input | 1 |
| `invalid_evidence` | 2 |
| `untrusted` | 3 |
| `pending` | 4 |
| `unverifiable` | 5 |

`unverifiable` is the archival gap: the RPC cannot return the transaction and cannot prove it failed. A transaction that is present and contradictory is `invalid_evidence`. Trust fields inside the bundle are ignored as anchors.
