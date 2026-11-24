# Phase A evidence

2026-11-24 · WP-A0 **PASS** on GitHub Actions, revision `b7416853a06c02cd7d22248db961f4926dcfd885`. The disposable validator initialized a new ledger. Program deployment and the separate client invocation both completed at finalized commitment. [Job and full logs](https://github.com/SampleBias/0x463/actions/runs/45425441171/job/138556028479)

| Hosted check | Command or value | Exit |
| --- | --- | ---: |
| Generate and resolve | `bash scripts/phase-a0-spike.sh` called `anchor init`, `cargo fetch`, `npm install` and `cargo fmt --all --check` | 0 each |
| Build | `anchor build --no-idl --tools-version v1.57 --arch v3 --provider.wallet "$wallet"` | 0 |
| Deploy | `solana program deploy target/deploy/settle463_spike.so --program-id "$program_key" --output json` against loopback | 0 |
| Invoke | `AnchorProvider.env().sendAndConfirm` with `commitment: "finalized"`, instruction `initialize` | 0 |
| Genesis hash | `FxtQ4wnmBScLRzfYzvpikvuUoSwUJh1mso5pbWPXu2gM` | |
| Program | `GUUpc14gv8pcf9vzs9rTratT4nzGdymS2226vFqNReu4` | |
| Deployment signature | `va1JQxB1YHYTz7jJihQUSyCpwfTv8bWWw3r3JSQiL5273MQMUDqnK9ehEbdguT6k6d1r7Y1XhmMuuZeAfSbN6i7` | |
| Invocation signature | `63Ko1gjfWcTujXVXh9cyuFJsJ2uMsXrqyZQEGeqHm7NjokDm54kfSCwgnvigWJrLeuNhvstidpxvS1CERuVH2rGW` | |

The project used host Rust/Cargo 1.89.0, Anchor 1.2.2, Solana CLI/validator 4.1.2, cargo-build-sbf 4.1.0, platform-tools v1.57 with Rust 1.95.0-dev and SBPF v3. The runner's default Rust 1.100.0 was superseded by the template's 1.89.0 pin. [Full configuration](../spec/toolchain.md)

## Hosted spike attempts

| Runner revision | Exact build | Outcome |
| --- | --- | --- |
| [ea36676](https://github.com/SampleBias/0x463/actions/runs/45418399739) | Anchor 0.32.1, Agave 2.3.0, bundled platform Cargo 1.84 | SBF compilation stopped on the edition-2024 manifest in `cpufeatures 0.3.1`. |
| [34a1fd0](https://github.com/SampleBias/0x463/actions/runs/45419885532) | Anchor 1.2.2, Agave 4.1.2, platform-tools v1.57, SBPF v3 | The compiler reached `anchor-spl`. Its reduced feature set was missing the mandatory Solana SDK selection. The next run explicitly includes `solana-v3`, matching the `anchor-lang` default. |

## Local toolchain and commands

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

The independent Node check below verifies both JSON spellings against the regression bytes. Exit 0, output `7b22e280a8223a22e280a9227d`. Rust conformance results are recorded below. The escrow-specific validator scenarios belong to Phase D.

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

## Independent worked fixtures

The worked payout uses the four published test identities, a 52-byte artifact and a terminal simulator clock of 1700001800. The bundle contains signed example data. No ledger transaction was submitted for this fixture.

| Command | Exit | Result |
| --- | ---: | --- |
| `cmp plan/golden/positive-signing-vector.json fixtures/golden/positive-signing-vector.json` | 0 | Published copy matches the input byte for byte. |
| `python3 fixtures/tools/independent_vector.py --write` | 0 | Generated the worked messages, bundle, ID index and ten negative cases. |
| `python3 fixtures/tools/independent_vector.py` | 0 | Reproduced the golden IDs, verified the offer, and matched the stored example files. This command uses Python and Ed25519 through cryptography, independently of Rust. |
| `python3 -O fixtures/tools/independent_vector.py` | 0 | The fixture checks also run with Python assertions disabled. |

A second implementation checks the shipped bytes using Node's Ed25519 verifier and SHA-256. The command below verifies all seven signatures, derives the order and policy identifiers from their signed inputs, and follows the message references through the paid receipt. It checks the buyer, seller, verifier and receipt issuer against the original golden public keys.

```sh
node --input-type=module <<'JS'
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createHash, createPublicKey, verify} from 'node:crypto';

const dir = 'spec/examples/worked-transaction/';
const read = file => JSON.parse(readFileSync(file, 'utf8'));
const canonical = v => {
  if (Array.isArray(v)) return '[' + v.map(canonical).join(',') + ']';
  if (v && typeof v === 'object') {
    return '{' + Object.keys(v).sort().map(k => JSON.stringify(k) + ':' + canonical(v[k])).join(',') + '}';
  }
  assert.ok(typeof v === 'string' || typeof v === 'boolean');
  return JSON.stringify(v);
};
const input = (domain, obj) => Buffer.from('0x463/' + domain + '/0.1\0' + canonical(obj));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const golden = read('plan/golden/positive-signing-vector.json');
const bundle = read(dir + 'bundle.json');
const roles = {service_offer:'seller', request:'buyer', quote:'seller', accept:'buyer',
              delivery:'seller', verdict:'verifier', receipt:'coordinator'};
const ids = {}, bodies = {};
for (const [kind, role] of Object.entries(roles)) {
  const message = read(dir + kind + '.json');
  const {signature, ...obj} = message;
  assert.equal(obj.issuer, golden.keys[role].public_key_hex);
  assert.deepEqual([obj.protocol,obj.version,obj.environment,obj.kind], ['0x463','0.1','sim:fixture',kind]);
  const key = createPublicKey({format:'der', type:'spki',
    key:Buffer.from('302a300506032b6570032100' + obj.issuer, 'hex')});
  const data = input('signed',obj);
  assert.ok(verify(null,data,key,Buffer.from(signature,'hex')),kind);
  ids[kind] = hash(data);
  bodies[kind] = obj.body;
  assert.ok(readFileSync(dir+'README.md','utf8').includes(ids[kind]));
  if (kind !== 'service_offer') assert.deepEqual(bundle[kind === 'accept' ? 'acceptance' : kind], message);
}
const {request:r, quote:q, accept:a, delivery:d, verdict:v, receipt:t} = bodies;
const order = hash(input('order',{buyer:r.buyer, environment:bundle.environment, nonce:r.nonce}));
const policy = hash(input('policy',q.acceptance_policy));
for (const body of [r,q,a,d,v,t,bundle]) assert.equal(body.order_id,order);
for (const body of [q,d,v]) assert.equal(body.policy_hash,policy);
for (const body of [r,q]) assert.equal(body.offer_id,ids.service_offer);
for (const body of [q,a,t]) assert.equal(body.request_id,ids.request);
for (const body of [a,d,v,t]) assert.equal(body.quote_id,ids.quote);
assert.equal(t.acceptance_id,ids.accept);
for (const body of [v,t]) assert.equal(body.delivery_id,ids.delivery);
assert.equal(t.verdict_id,ids.verdict);
const artifact = readFileSync('fixtures/artifacts/reference.json');
for (const digest of [d.digest,v.observed_digest,q.artifact.digest,q.acceptance_policy.expected_digest])
  assert.equal(digest,hash(artifact));
for (const length of [d.length,v.observed_length,q.artifact.length,q.acceptance_policy.expected_length])
  assert.equal(length,String(artifact.length));
assert.deepEqual([v.decision,v.reason_codes,t.outcome],['PASS',['MATCH'],'paid']);
assert.equal(t.destination,'sim:'+golden.keys.seller.public_key_hex);
assert.equal(t.principal,'2000000');
for (const field of ['destination','principal','finality','ledger_kind','terminal_locator'])
  assert.equal(bundle.ledger[field],t[field]);
assert.ok(BigInt(bundle.ledger.clock) < BigInt(q.deadlines.resolve_by));
console.log('7 signatures and linked IDs verified; receipt_id=' + ids.receipt);
JS
```

Expected receipt ID is `22eb00b33ea53cb39763aa1907dc35bc47327d115f5e2c8d148fef447a5a627b`. The worked transaction index contains the remaining message IDs.

## Phase A Rust workspace

The root workspace contains the eight required members. Phase A implements wire processing in `settle463-core`. The other seven members compile as libraries and document their implementation phase. All eight participate in host checks. The escrow contract and its economic behavior start in Phase D.

The following checks ran locally on host Rust/Cargo 1.89.0 on 2026-11-24. Cargo used `CARGO_HOME=/tmp/settle463-domain-cargo` and the committed lockfile. Two public crate archives missing locally were fetched on the [dependency runner](https://github.com/SampleBias/0x463/actions/runs/45427350037) and verified against the registry SHA-256 values in `Cargo.lock` before the offline build.

| Command | Exit | Result |
| --- | ---: | --- |
| `cargo generate-lockfile --offline` | 0 | Registry dependencies resolved. |
| `cargo fmt --all --check` | 0 | All eight members formatted. |
| `cargo check --locked --offline --workspace` | 0 | All eight members compile. |
| `cargo check --locked --offline -p settle463-core -p settle463-verify -p settle463-sim -p settle463-sdk` | 0 | Required domain crates compile. |
| `cargo clippy --offline -p settle463-core --all-targets -- -D warnings` | 0 | Library and test targets pass. |
| `cargo test --locked --offline -p settle463-core` | 0 | All eight integration tests pass. Covers T-GOLD-01–04, T-WIRE-01, all seven worked IDs, contextual amount rejection, positional JSON and missing verdict observations. |
| `cmp plan/golden/positive-signing-vector.json fixtures/golden/positive-signing-vector.json` | 0 | Exact copy. |
| `python3 fixtures/tools/independent_vector.py` | 0 | T-GOLD-05. Reproduced the original IDs and signature, seven worked messages, ten rejection fixtures and six raw artifacts. |
| `cargo tree --locked --offline -p settle463-core -e normal` | 0 | The core has no Solana, Anchor, Tokio, Axum or SQLx dependency. |

The Phase A [domain workflow](../.github/workflows/ci.yml) runs formatting, Clippy, locked Rust tests and the independent Python check on pushes and pull requests. Its run and commit can be located together in [the branch's run history](https://github.com/SampleBias/0x463/actions/workflows/ci.yml?query=branch%3Abuild%2Fphase-a-domain).
