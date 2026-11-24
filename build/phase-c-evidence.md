# Phase C simulator evidence

The four scenarios below are **simulator verified**. The hosted Linux runner built revision `0e9b76c921fc1592f49f9276022aedafde3d89d3` on Rust 1.89.0, based on the Phase B main revision `60ae0ac7dfda1f01be580e4046ea536aed64262e`. Each CLI started a fresh SQLite-backed coordinator on loopback and negotiated through the public HTTP SDK. All four demo, offline verification, and order inspection processes exited 0. Each verifier printed `evidence_signed`. [Domain conformance run](https://github.com/SampleBias/0x463/actions/runs/45449192303) · [Public receipts, detached trust inputs and transcripts](https://github.com/SampleBias/0x463/actions/runs/45449192303/artifacts/14948391236)

## Scenario transcripts

The paths below are the exact demo output on the runner. The retained archive stores each bundle at `<scenario>/receipt.json` beside `trust.json`, `demo.stdout`, `verify.stdout`, and `inspect.stdout`. Deltas are in atomic units of the six-decimal simulator asset.

### pass

```text
order_id=71fdf7163e6b22aaee958c74183757d1fa209574ffbada63a27555662a463640
backend=sim
outcome=paid
asset=test-six-decimal
principal=2000000
delta_buyer=-2000000
delta_seller=2000000
delta_escrow=0
receipt=/tmp/settle463-cli-jp_w0ugp/pass/receipt.json
```

### bad-artifact

```text
order_id=e9b2ffef78b3338115a2e7f9283f55548047d6dc4ef91ceec620855d7748edb6
backend=sim
outcome=refunded_timeout_after_delivery
asset=test-six-decimal
principal=2000000
delta_buyer=0
delta_seller=0
delta_escrow=0
receipt=/tmp/settle463-cli-jp_w0ugp/bad-artifact/receipt.json
```

### no-delivery

```text
order_id=b2422d682ca822ad1586f83a0c7fdac14e54bd290b99db11f0a509ac42d75020
backend=sim
outcome=refunded_timeout_before_delivery
asset=test-six-decimal
principal=2000000
delta_buyer=0
delta_seller=0
delta_escrow=0
receipt=/tmp/settle463-cli-jp_w0ugp/no-delivery/receipt.json
```

### verifier-offline

```text
order_id=a93d63cd248811ecd28e237efd6eb55133783f31810c4a74b0224ecaf4e37061
backend=sim
outcome=refunded_timeout_after_delivery
asset=test-six-decimal
principal=2000000
delta_buyer=0
delta_seller=0
delta_escrow=0
receipt=/tmp/settle463-cli-jp_w0ugp/verifier-offline/receipt.json
```

## Conformance checks

| Cases | Observed result |
| --- | --- |
| T-ART-01 | The pass buyer downloaded the 52 reference bytes through the SDK. SHA-256 was `37ba5c2e570e4e256af7049d0d9c27f70b50f5de4210101d7c0bd2b497db7587`. |
| T-ART-02 | A separate funded order committed to the digest and length of the 64-byte extra-field fixture. The signed verdict contained exactly `FAIL` and `SCHEMA_MISMATCH`. Release was denied, the seller received zero, and the buyer recovered the principal at timeout. |
| T-ART-03–05 | The verifier rejected malformed, duplicate-key and oversized artifacts. The service rejected cross-order reads, returned the same content ID for identical bytes, and preserved the original when different bytes arrived. |
| T-TIME-02–05 and T-IDEM-01–05, 09 | Boundary, replay and competing release/refund tests checked when delivery or settlement was legal and that a terminal transfer happened at most once. |
| T-IDEM-06–08 | The daemon replayed the original response for an identical HTTP request, rejected a reused key with a different body, and preserved one deposit with a fresh key. |
| T-VER-01, 02, 06, 07 | A changed signature returned exit 2. An untrusted issuer returned exit 3. The timeout-before-delivery receipt omitted both delivery and verdict; the silent-verifier receipt included only delivery. Both verified. |
| T-REC-01 | The debug process stopped after committing the funding intent and outbox entry. Two subsequent inspect processes restarted the same database. Both read `Funded` with buyer balance 3000000, held 2000000, seller balance 0, paid 0 and refunded 0. |
| Direct exchange | Two separately keyed coordinators exchanged and returned the original signed quote bytes without discovery. |
| Approval and evidence binding | Failed verdicts carrying observations about a different digest or length were rejected even with a valid verifier signature. |

The hosted job passed formatting, warnings-denied Clippy, 35 Rust tests across the six Phase C packages, the independent fixture generator and the four CLI scenarios. Cargo ran offline for the tests and CLI scenarios after dependencies were cached. The local sandbox denied socket creation, so the hosted run provides the process and loopback results. [Job log](https://github.com/SampleBias/0x463/actions/runs/45449192303/job/138641298763)
