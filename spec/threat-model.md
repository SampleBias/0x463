# Trust and failure modes

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

Acceptance means the agreed verifier observed bytes that match the committed rules. In the reference implementation the buyer can download those same bytes. A verdict does not prove legal rights, secrecy, semantic quality, or real-world completion. Profile `sha256-json-object-v1` fits artifacts with a known digest or reproducible expected output.

Every agent validates the transaction it is asked to sign, including every instruction and every account destination. An independent participant who has the signed order bundle can inspect the escrow and invoke an eligible refund through the SDK while the coordinator is offline.

An unavailable relay can prevent delivery or retrieval until a timeout permits a refund. The coordinator can withhold messages or replace its entire local audit log. Its signed observations authenticate what it reported. A receipt verifier supplies its own issuer, order and deployment trust inputs. Only online verification against the pinned ledger establishes finalized payment.

The devnet upgrade authority can change the program after deployment. The ledger rules also depend on a functioning chain, a transferable token and a caller to submit the eligible transaction. [Receipt verification](receipts.md#verification-modes) · [Deployment configuration](settlement-solana.md#deployment-configuration)
