# Rejected fixture messages

Use `sim:fixture` as the pinned environment and the [worked offer, request and quote](../worked-transaction/README.md) as the linkage context. Each file replaces the corresponding message for its case. Read the duplicate-key file as raw bytes.

| Input | Expected rejection |
| --- | --- |
| [Uppercase hex](uppercase-hex.json) | The signature uses uppercase hex. |
| [Duplicate key](duplicate-key.json) | The outer object repeats `version` with identical values. Reject before deserializing into a map. |
| [Unknown field](unknown-field.json) | The signed envelope contains the extra field `extra`. |
| [Wrong version](wrong-version.json) | The envelope declares version `0.2`. |
| [Wrong environment](wrong-environment.json) | This valid offer belongs to `sim:other`. |
| [Number token](number-token.json) | `expires_at` is a JSON number. Signed timestamps require decimal strings. |
| [Seller as verifier](seller-as-verifier.json) | The seller occupies two roles and falls outside the request's verifier allowlist. |
| [Above ceiling](above-ceiling.json) | The signed quote asks for `2500001`. The worked buyer request caps principal at `2500000`. |
| [Equal deadlines](equal-deadlines.json) | Both `fund_by` and `deliver_by` are `1700003600`. |
| [Wrong signature](wrong-signature.json) | The one-byte change to the service ID invalidates the original seller signature. |
