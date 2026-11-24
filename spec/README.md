# 0x463 protocol 0.1

A buyer and a seller agree on a fixed price for one artifact. Both authorize funding. The seller commits its delivery, and a distinct verifier can authorize payment before the resolution deadline. Any caller can submit an eligible timeout refund. The coordinator relays messages and signs receipts. Protocol fees are zero.

`0x463` is the brand string. Deployments use their own environment identifiers. The Rust package prefix is `settle463`.

A dishonest verifier can approve bad work. If the verifier is unavailable after delivery, the buyer can recover the principal at `resolve_by`. The buyer can also recover it at `deliver_by` if the seller never committed a delivery.

| Contract | Coverage |
| --- | --- |
| [Wire format](wire.md) | Signed envelopes, canonical bytes, identifiers and message linkage |
| [State machine](state-machine.md) | Authorizations, deadlines, replay and principal conservation |
| [Threat model](threat-model.md) | Role keys, verifier trust, availability and upgrade authority |
| [Artifact verifier](verifier-sha256-json-object-v1.md) | Raw digest, schema, limits and verdict reasons |
| [HTTP API](api-v1.md) | Routes, challenges, private reads and idempotency |
| [Persistence](persistence.md) | Original bytes, audit events, intents, outbox and restart |
| [Receipts](receipts.md) | Bundle, outcomes, trust inputs and verification results |
| [Simulator](settlement-sim.md) | Injected clock and six-decimal test asset |
| [Solana](settlement-solana.md) | On-chain accounts, instructions, SPL transfers and reconciliation |
| [Build toolchain](toolchain.md) | Candidate versions and local deployment check |

The [worked transaction](examples/worked-transaction/README.md) contains deterministic signed fixture messages for one simulated payout. The [negative examples](examples/negative/README.md) identify the input and context for each rejection.
