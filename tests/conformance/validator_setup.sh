#!/usr/bin/env bash
set -euo pipefail
umask 077
if (( $# < 2 )); then
  echo "usage: validator_setup.sh <escrow.so> <test command> [args...]" >&2
  exit 2
fi
program_so=$(realpath "$1")
shift
root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
program=$(sed -n 's/^declare_id!("\([^"]*\)");/\1/p' "$root/programs/settle463-escrow/src/lib.rs")
test -n "$program" && test -f "$program_so"
fixture=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/settle463-validator.XXXXXXXX")
validator_pid=
cleanup() {
  status=$?
  if [[ -n "$validator_pid" ]]; then
    if (( status != 0 )); then tail -50 "$fixture/validator.log" >&2; fi
    kill "$validator_pid" 2>/dev/null || true
    wait "$validator_pid" 2>/dev/null || true
  fi
  rm -rf -- "$fixture"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for role in buyer seller verifier coordinator fee-payer mint-authority mint; do
  solana-keygen new --silent --no-bip39-passphrase --outfile "$fixture/$role.json" >/dev/null
done
payer="$fixture/fee-payer.json"
port=$(python3 "$root/scripts/validator_fixture.py" port)
rpc="http://127.0.0.1:$port"
solana-test-validator --ledger "$fixture/ledger" --bind-address 127.0.0.1 \
  --rpc-port "$port" --faucet-port "$((port+2))" --mint "$(solana-keygen pubkey "$payer")" \
  --upgradeable-program "$program" "$program_so" "$(solana-keygen pubkey "$payer")" \
  >"$fixture/validator.log" 2>&1 &
validator_pid=$!
cli=(solana --url "$rpc" --ws "ws://127.0.0.1:$((port+1))" --keypair "$payer" --commitment finalized)
for ((i=0; i<90; i++)); do
  kill -0 "$validator_pid"
  if "${cli[@]}" account "$(solana-keygen pubkey "$payer")" >/dev/null 2>&1; then break; fi
  sleep 1
done
((i<90))
"${cli[@]}" genesis-hash
# Genesis pins the program address without distributing its private key. Submit the exact built ELF through the upgrade authority.
"${cli[@]}" program deploy "$program_so" --program-id "$program" --use-rpc --output json
"${cli[@]}" program show "$program" --output json
python3 "$root/scripts/validator_fixture.py" prepare "$fixture" "$rpc" "$program"
export PHASE_D_FIXTURE="$fixture" PHASE_D_DEPLOYMENT="$fixture/deployment.json"
"$@"
