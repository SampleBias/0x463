# Worked fixture transaction

Seven fixture messages describe a simulated payout of 2000000 atomic units in `sim:fixture`. The raw artifact is [reference.json](../../../fixtures/artifacts/reference.json). The fixture assigns content ID `22` repeated 32 times. Funding, delivery and release are the three modeled ledger events. The terminal clock is 1700001800 and the bundle has an empty observations array.

| Signed message | Message ID |
| --- | --- |
| [service_offer](service_offer.json) | `ac78a7245326764d548f13907645c1ffcaf926da6a24292f4ca480e45d668004` |
| [request](request.json) | `02dc20c9d5f3f3bfaba6d91d97acd7ba10d25977988d2278a65468a7a74f1732` |
| [quote](quote.json) | `897f47f4c14d7c93cb1ab3696b238af74c647a8cd59c0a35841a062069407003` |
| [accept](accept.json) | `56f8fa53822285761b6f83865c0c1d91abd19ae86dc426880b6820c41807908f` |
| [delivery](delivery.json) | `e16aaf1b7c9fb3296e8a6ed5b6d8644534fd5ffc7ef3b731b73892483090e4ff` |
| [verdict](verdict.json) | `436e93d42c6fa3e742623b1562c4edc4279a30736080408ce5bf4db2f435759b` |
| [receipt](receipt.json) | `22eb00b33ea53cb39763aa1907dc35bc47327d115f5e2c8d148fef447a5a627b` |

The [bundle](bundle.json) packages the signed evidence. Its receipt has the message ID shown above.
