#!/usr/bin/env bash
set -euo pipefail

anchor_version=$(anchor --version)
solana_version=$(solana --version)
validator_version=$(solana-test-validator --version)
sbf_version=$(cargo-build-sbf --version)
printf '%s\n' "$anchor_version" "$solana_version" "$validator_version" "$sbf_version"
if [[ $anchor_version != 'anchor-cli 0.32.1' ||
      ${solana_version%% (*} != 'solana-cli 2.3.0' ||
      ${validator_version%% (*} != 'solana-test-validator 2.3.0' ||
      ${sbf_version%%$'\n'*} != 'solana-cargo-build-sbf 2.3.0' ]]; then
    echo 'This spike tests Anchor 0.32.1 with the Agave 2.3.0 tool suite.' >&2
    exit 2
fi

umask 077
scratch=$(mktemp -d "${TMPDIR:-/tmp}/settle463-a0.XXXXXXXX")
echo "Spike directory: $scratch"
cd "$scratch"
wallet="$scratch/payer.json"
rpc=http://127.0.0.1:19299
solana-keygen new --silent --no-bip39-passphrase --outfile "$wallet"
payer=$(solana-keygen pubkey "$wallet")
anchor init settle463_spike --no-install --no-git --javascript --package-manager npm
cd settle463_spike
rustc --version
cargo --version
# The generated Cargo requirement is a range. Exercise the CLI's exact crate version.
manifest=programs/settle463_spike/Cargo.toml
sed -i 's/^anchor-lang = "0\.32\.1"$/anchor-lang = "=0.32.1"/' "$manifest"
grep -Fxq 'anchor-lang = "=0.32.1"' "$manifest"
cat >>"$manifest" <<'TOML'

[dependencies.anchor-spl]
version = "=0.32.1"
default-features = false
features = ["token", "associated_token"]
TOML
cargo fetch
npm install --omit=dev --save-exact --no-audit --no-fund @coral-xyz/anchor@0.32.1
cargo fmt --all --check
anchor build --provider.wallet "$wallet"

solana-test-validator --ledger "$scratch/ledger" --bind-address 127.0.0.1 \
    --rpc-port 19299 --faucet-port 19298 --mint "$payer" \
    >"$scratch/validator.log" 2>&1 &
validator_pid=$!
stop_validator() {
    kill "$validator_pid" 2>/dev/null || true
    wait "$validator_pid" 2>/dev/null || true
}
trap stop_validator EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cli=(solana --url "$rpc" --ws ws://127.0.0.1:19300 --keypair "$wallet" --commitment finalized)
for ((attempt = 0; attempt < 60; attempt++)); do
    if ! kill -0 "$validator_pid" 2>/dev/null; then
        cat "$scratch/validator.log" >&2
        exit 1
    fi
    # Only the fresh ledger has the newly generated payer funded at genesis.
    if "${cli[@]}" account "$payer" >/dev/null 2>&1; then
        break
    fi
    sleep 1
done
if ((attempt == 60)); then
    echo "Fresh validator did not become ready. See $scratch/validator.log" >&2
    exit 1
fi

"${cli[@]}" genesis-hash
"${cli[@]}" program deploy target/deploy/settle463_spike.so \
    --program-id target/deploy/settle463_spike-keypair.json --output json
ANCHOR_WALLET="$wallet" ANCHOR_PROVIDER_URL="$rpc" node <<'JS'
const anchor = require("@coral-xyz/anchor");
async function invoke() {
    const provider = anchor.AnchorProvider.env();
    const program = new anchor.Program(require("./target/idl/settle463_spike.json"), provider);
    const signature = await program.methods.initialize().rpc({commitment: "finalized"});
    console.log("initialize", signature);
}
invoke().catch(error => { console.error(error); process.exitCode = 1; });
JS
