# Phase C simulator evidence

The four scenarios below are **simulator verified**. On 2026-11-24 the hosted Linux runner built revision `f898112ec31d457382673f1954352c96a2f260f3` on Rust 1.89.0, based on the Phase B main revision `60ae0ac7dfda1f01be580e4046ea536aed64262e`. Each CLI started a fresh SQLite-backed coordinator on loopback and negotiated through the public HTTP SDK. All four demo, offline verification, and order inspection processes exited 0. Each verifier printed `evidence_signed`. [Domain conformance run](https://github.com/SampleBias/0x463/actions/runs/45452163907) · [Public receipts, detached trust inputs and transcripts](https://github.com/SampleBias/0x463/actions/runs/45452163907/artifacts/14946904053)

## Scenario transcripts

The paths below are the exact demo output on the runner. The retained archive stores each bundle at `<scenario>/receipt.json` beside `trust.json`, `demo.stdout`, `verify.stdout`, and `inspect.stdout`. Deltas are in atomic units of the six-decimal simulator asset.

### pass

```text
order_id=cc6bb804d47cd439e430eb5091598adf885505f5e88057cd4e4cafec5e01ad69
backend=sim
outcome=paid
asset=test-six-decimal
principal=2000000
delta_buyer=-2000000
delta_seller=2000000
delta_escrow=0
receipt=/tmp/settle463-cli-tkem_26l/pass/receipt.json
```

### bad-artifact

```text
order_id=51cbb612e9792b4c882195117a9e2593c22af90765e28354a77552cc30e929bb
backend=sim
outcome=refunded_timeout_after_delivery
asset=test-six-decimal
principal=2000000
delta_buyer=0
delta_seller=0
delta_escrow=0
receipt=/tmp/settle463-cli-tkem_26l/bad-artifact/receipt.json
```

### no-delivery

```text
order_id=50a3a583f7785cf564c4c5d9e0822f10ba3dc5acee432ee6978215e182903d77
backend=sim
outcome=refunded_timeout_before_delivery
asset=test-six-decimal
principal=2000000
delta_buyer=0
delta_seller=0
delta_escrow=0
receipt=/tmp/settle463-cli-tkem_26l/no-delivery/receipt.json
```

### verifier-offline

```text
order_id=a7114568a37a1cde492a8bf18e0c1a886ff9e9b55fc591db188a10fdcac02f9e
backend=sim
outcome=refunded_timeout_after_delivery
asset=test-six-decimal
principal=2000000
delta_buyer=0
delta_seller=0
delta_escrow=0
receipt=/tmp/settle463-cli-tkem_26l/verifier-offline/receipt.json
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
| Native SQLite | CI built SQLite 3.54.1 from its checksum-pinned upstream source with column metadata and unlock notification enabled. The daemon checked the loaded library against its 3.54.0 minimum before opening its database. |
| Direct exchange | Two separately keyed coordinators exchanged and returned the original signed quote bytes without discovery. |
| Approval and evidence binding | Failed verdicts carrying observations about a different digest or length were rejected even with a valid verifier signature. |

The hosted job passed formatting, warnings-denied Clippy, 36 Rust tests across the six Phase C packages, the independent fixture generator and the four CLI scenarios. Cargo ran offline for the tests and CLI scenarios after dependencies were cached. The local sandbox denied socket creation, so the hosted run provides the process and loopback results. [Job log](https://github.com/SampleBias/0x463/actions/runs/45452163907/job/138651877646)
