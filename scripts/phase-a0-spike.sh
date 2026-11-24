#!/usr/bin/env bash
set -euo pipefail

anchor_version=$(anchor --version)
solana_version=$(solana --version)
validator_version=$(solana-test-validator --version)
sbf_version=$(cargo-build-sbf --version)
sbf_first_line=${sbf_version%%$'\n'*}
printf '%s\n' "$anchor_version" "$solana_version" "$validator_version" "$sbf_version"
if [[ $anchor_version != 'anchor-cli 1.2.2' ||
      ${solana_version%% (*} != 'solana-cli 4.1.2' ||
      ${validator_version%% (*} != 'solana-test-validator 4.1.2' ||
      ${sbf_first_line##* } != 4.1.0 ]]; then
    echo 'This spike tests Anchor 1.2.2 with the Agave 4.1.2 tool suite.' >&2
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
export NO_DNA=1
anchor init settle463_spike --no-install --no-git --javascript --package-manager npm --template single --test-template jest
cd settle463_spike
rustc -Vv
cargo --version
# The generated Cargo requirement is a range. Exercise the CLI's exact crate version.
manifest=programs/settle463_spike/Cargo.toml
sed -i 's/^anchor-lang = "1\.2\.2"$/anchor-lang = "=1.2.2"/' "$manifest"
grep -Fxq 'anchor-lang = "=1.2.2"' "$manifest"
cat >>"$manifest" <<'TOML'

[dependencies.anchor-spl]
version = "=1.2.2"
default-features = false
features = ["token", "associated_token"]
TOML
program_key=target/deploy/settle463_spike-keypair.json
program_id=$(solana-keygen pubkey "$program_key")
cat > programs/settle463_spike/src/lib.rs <<RS
use anchor_lang::prelude::*;

declare_id!("$program_id");

#[program]
pub mod settle463_spike {
    use super::*;

    pub fn initialize(_ctx: Context<Initialize>) -> Result<()> {
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
RS
cargo fetch
npm install --omit=dev --save-exact --no-audit --no-fund @anchor-lang/core@1.2.2
cargo fmt --all --check
anchor build --no-idl --tools-version v1.57 --arch v3 --provider.wallet "$wallet"
"$HOME/.cache/solana/v1.57/platform-tools/rust/bin/rustc" -Vv

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
    --program-id "$program_key" --output json
SPIKE_PROGRAM_ID="$program_id" ANCHOR_WALLET="$wallet" ANCHOR_PROVIDER_URL="$rpc" node <<'JS'
const anchor = require("@anchor-lang/core");
const { createHash } = require("node:crypto");
async function invoke() {
    const provider = anchor.AnchorProvider.env();
    const { PublicKey, Transaction, TransactionInstruction } = anchor.web3;
    const instruction = new TransactionInstruction({
        programId: new PublicKey(process.env.SPIKE_PROGRAM_ID),
        keys: [],
        data: createHash("sha256").update("global:initialize").digest().subarray(0, 8),
    });
    const signature = await provider.sendAndConfirm(new Transaction().add(instruction), [], {
        commitment: "finalized",
        preflightCommitment: "confirmed",
    });
    console.log("initialize", signature);
}
invoke().catch(error => { console.error(error); process.exitCode = 1; });
JS
