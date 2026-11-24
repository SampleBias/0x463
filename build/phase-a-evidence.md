# Phase A evidence

2026-11-24 · WP-A0 **BLOCKED**. Git revision unavailable. The supplied directory contains the plan and has no readable Git HEAD.

A one-instruction Anchor program and its client were generated under `/tmp`. Formatting passed. The build stopped during dependency resolution and the validator stopped during socket creation. Deployment and invocation remain pending. Phase A requires both before WP-A2. The workspace checks, Rust golden tests and subsequent phases remain pending.

## Toolchain and commands

| Tool | Observed version |
| --- | --- |
| Host rustc / cargo | 1.97.0 / 1.97.0 |
| Anchor template rustc / cargo | 1.89.0 / 1.89.0 |
| Anchor CLI | 0.32.1 |
| Active Solana CLI / validator / cargo-build-sbf | 3.1.8 / 3.1.8 / 3.1.8 |
| Installed active platform-tools / SBF rustc | v1.52 / 1.89.0 |
| Candidate Solana CLI / validator | 2.3.0, unavailable locally |

The scratch parent for the diagnostic run was `/tmp/settle463-a0.UAXU4HaX`. Commands in the program directory ran in its `settle463_spike` subdirectory. See [toolchain selection and the reproduction procedure](../spec/toolchain.md).

| Working directory | Command | Exit | Result |
| --- | --- | ---: | --- |
| Project | `git rev-parse --verify HEAD` | 128 | Git could not discover a repository. |
| Project | `rustc --version`, `cargo --version`, `anchor --version`, `solana --version`, `solana-test-validator --version`, `cargo-build-sbf --version` | 0 each | Versions above. |
| Project | `curl -fIL --max-time 12 https://release.anza.xyz/v2.3.0/install` | 6 | Could not resolve release.anza.xyz. |
| Project | `curl --fail --head --max-time 12 https://index.crates.io/an/ch/anchor-lang` | 6 | Could not resolve index.crates.io. |
| Scratch parent | `anchor init settle463_spike --no-git --no-install --template single` | 0 | Generated a single `initialize` instruction returning `Ok(())` and an invocation test. |
| Program | `rustc --version`, `cargo --version`, `cargo fmt --all --check` | 0 each | Generated template selects Rust 1.89.0. Source formatting passed. |
| Program | `cargo-build-sbf --offline --manifest-path programs/settle463_spike/Cargo.toml` | 1 | Dependency resolution needed uncached `anchor-attribute-access-control v0.32.1`. |
| Scratch parent | `timeout 15s solana-test-validator --ledger /tmp/settle463-a0.UAXU4HaX/ledger --bind-address 127.0.0.1 --rpc-port 19299 --faucet-port 19298` | 1 | The installed validator reported `Unable to bind faucet to 0.0.0.0:19298` and `Operation not permitted (os error 1)`. |
| Project | `python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0))'` | 1 | Socket creation raised `PermissionError` before binding an ephemeral loopback port. The restriction applies independently of the validator version. |
| Project | `bash -n scripts/phase-a0-spike.sh` | 0 | Shell syntax passed. |
| Project | `bash scripts/phase-a0-spike.sh` | 2 | The candidate requires the 2.3.0 suite. Found 3.1.8. |
| Project | `CARGO_TARGET_DIR=/tmp/settle463-a0.UAXU4HaX/vers-target bash /home/s4mpl3bi4s/.codex/skills/vers/scripts/verify.sh --root /home/s4mpl3bi4s/0x463 --full` | 2 | VERS v0.2.0 recorded `BLOCKED` at `G0-DISCOVERY`. It requires a Git-root Cargo workspace. The run produced discovery evidence only. |

The local probes used the installed suite to distinguish missing dependencies and socket restrictions. Neither a build artifact nor a ledger transaction was available for assessing SBPF compatibility.

## Provisioning retry

The installer targeted a side-by-side directory. In the following commands, `tools_dir=/home/s4mpl3bi4s/.cache/rust-tmp/settle463-agave-2.3.0` and `probe=/tmp/settle463-deps.l0UNYMyO`. The template manifest requested exactly `anchor-lang` and `anchor-spl` 0.32.1, with the `token` and `associated_token` features for SPL. The JS package was requested separately so each registry could report its own result.

| Working directory | Command | Exit | Result |
| --- | --- | ---: | --- |
| Project | `agave-install-init --no-modify-path --config "$tools_dir/config.yml" --data-dir "$tools_dir/install" 2.3.0` | 1 | The installer failed to fetch the official Linux release archive. |
| Project | `curl -fL --connect-timeout 4 --max-time 15 --output "$probe/solana-release.tar.bz2" https://github.com/anza-xyz/agave/releases/download/v2.3.0/solana-release-x86_64-unknown-linux-gnu.tar.bz2` | 6 | Could not resolve github.com. No 2.3.0 installation was available afterward. |
| Probe | `anchor init spike_deps --no-install --no-git --javascript --package-manager npm` | 0 | Generated the scratch program used by the dependency probes. |
| Probe program | `cargo metadata --offline --no-deps --format-version 1` | 0 | Manifest requirements were both `=0.32.1`. SPL selected `token` and `associated_token`. |
| Probe program | `CARGO_HOME="$probe/cargo-home" CARGO_NET_RETRY=0 cargo fetch` | 101 | Could not resolve index.crates.io. |
| Probe program | `cargo fetch --offline` | 101 | Cargo resolved registry metadata, then required the missing archive for `anchor-spl v0.32.1`. |
| Fresh client directory | `npm install --save-exact --fetch-retries=0 --fetch-timeout=5000 --no-audit --no-fund --cache "$probe/npm-cache" @coral-xyz/anchor@0.32.1` | 1 | npm returned `EAI_AGAIN` for registry.npmjs.org. |
| Project | `python3 -c 'import socket; socket.socket().bind(("127.0.0.1", 0))'` | 1 | The sandbox denied socket creation. |

## Specification resolutions

1. **Signing interoperability.** Property names and string values emit literal U+2028 and U+2029. T-GOLD-03 now checks the exact bytes for escaped and literal input. This aligns signatures across RFC 8785 implementations. [Wire contract](../spec/wire.md#canonicalization-and-signatures) · [Regression vector](../plan/golden/jcs-line-separators.json)
2. **Ledger reconciliation.** The adapter checks funding, delivery, release and refund against the accounts, IDs and token movement for that action. Recovery accepts a later compatible escrow state as corroboration and requires the actual terminal transfer before issuing a receipt. T-REC-02 and T-VER-05 cover lost responses and missing history. [Solana contract](../spec/settlement-solana.md#adapter-reconciliation) · [Persistence contract](../spec/persistence.md#recovery-procedure)
3. **Stable errors.** A new zero-principal fund returns 6008. A mismatch in a positive principal transfer returns 6003. T-SOL-07 checks 6008 and rollback. The numeric assignments match the original error table. [Error contract](../spec/settlement-solana.md#custom-errors) · [Conformance cases](../plan/03-conformance-matrix.md#solana-constraints)

The independent Node check below verifies both JSON spellings against the regression bytes. Exit 0, output `7b22e280a8223a22e280a9227d`. The Rust conformance cases and validator scenarios await their implementation phases.

```sh
node <<'JS'
const assert = require('node:assert/strict');
const vector = require('./plan/golden/jcs-line-separators.json');
const literal = Buffer.from(vector.canonical_utf8_hex, 'hex').toString('utf8');
for (const input of [vector.input_json, literal]) {
    const object = JSON.parse(input);
    assert.deepEqual(object, { '\u2028': '\u2029' });
    assert.equal(Buffer.from(JSON.stringify(object)).toString('hex'), vector.canonical_utf8_hex);
}
console.log(vector.canonical_utf8_hex);
JS
```

## Planning fixture check

From the project root, the following read-only check uses Python 3.14.8, hashlib, and cryptography 50.0.2. It covers the supplied ASCII signing, order, and policy objects. The Rust implementation and conformance gates are separate work.

```sh
python3 - <<'PY'
import hashlib
import json
import re
from pathlib import Path
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

golden = json.loads(Path("plan/golden/positive-signing-vector.json").read_bytes())
for section, domain, field in [
    ("signing", "signed", "message_id"),
    ("order", "order", "order_id"),
    ("policy", "policy", "policy_hash"),
]:
    vector = golden[section]
    obj = vector["unsigned_object" if section == "signing" else "object"]
    canonical = json.dumps(obj, sort_keys=True, separators=(",", ":")).encode("ascii")
    preimage = f"0x463/{domain}/0.1".encode() + b"\x00" + canonical
    assert canonical.decode() == vector["canonical_utf8"]
    assert preimage.hex() == vector["preimage_hex"]
    digest = hashlib.sha256(preimage).hexdigest()
    assert digest == vector[field]
    print(field, digest)
    if section == "signing":
        key = Ed25519PublicKey.from_public_bytes(bytes.fromhex(obj["issuer"]))
        signature = bytes.fromhex(vector["signature_hex"])
        key.verify(signature, preimage)
        mutated = preimage.replace(b"sample-json-report", b"sample-json-reporu")
        assert sum(a != b for a, b in zip(preimage, mutated, strict=True)) == 1
        assert hashlib.sha256(mutated).hexdigest() != digest
        try:
            key.verify(signature, mutated)
        except InvalidSignature:
            print("original signature verified; one-byte mutation rejected")
        else:
            raise ValueError("mutated message verified")

plan = Path("plan/00-locked-decisions.md").read_text()
rows = re.findall(r"^\| [^|]+ \| `([^|]+)` \| (\d+) \| `([0-9a-f]{64})` \|$", plan, re.M)
assert len(rows) == 6
for raw, length, digest in rows:
    data = raw.encode("utf-8")
    assert len(data) == int(length) and hashlib.sha256(data).hexdigest() == digest
print("6 artifact lengths and digests matched")
PY
```

Exit 0. Output:

```text
message_id ac78a7245326764d548f13907645c1ffcaf926da6a24292f4ca480e45d668004
original signature verified; one-byte mutation rejected
order_id 8ed07cfeafd6b86b5c1a4501912ec6f1984442f1247bd79aca2a2f166433b51c
policy_hash caadaf810c19a779b80c08c6493ec2ecfde3fb8f56f15baa449b7897db2a5f4b
6 artifact lengths and digests matched
```
