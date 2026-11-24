# Phase A toolchain spike

**WP-A0 is blocked before deployment.** The candidate pairs Anchor CLI 0.32.1 with the Agave 2.3.0 CLI, validator and SBF builder. The scratch manifest selects exactly `anchor-lang` and `anchor-spl` 0.32.1 with classic SPL Token and associated token support. The invocation client uses exactly `@coral-xyz/anchor@0.32.1`. [Anchor 0.32.1 release notes](https://www.anchor-lang.com/docs/updates/release-notes/0-32-1#recommended-solana-version)

| Tool | Observed on 2026-11-24 |
| --- | --- |
| Host Rust and Cargo in the project directory | rustc 1.97.0 and cargo 1.97.0, x86_64-unknown-linux-gnu |
| Host Rust and Cargo inside the Anchor-generated scratch project | rustc 1.89.0 and cargo 1.89.0, selected by the template |
| Anchor CLI | 0.32.1 |
| Active Solana CLI and validator | Agave 3.1.8, revision `2717084a` |
| Other locally installed Solana CLI | 3.0.15 |
| Active cargo-build-sbf | 3.1.8 |
| Anchor Rust dependencies | Exact versions requested. The local Cargo cache lacks required crates, including `anchor-spl` 0.32.1. |
| Node / npm | 26.12.0 / 11.6.2. Fetching the Anchor JS package failed during registry DNS resolution. |
| Platform tools reported by the active builder | v1.52, rustc 1.89.0 |
| Candidate SBF platform tools | Agave 2.3.0 defaults to v1.48. The local platform-tools caches contain v1.51 and v1.52. [Agave 2.3.0 builder source](https://github.com/anza-xyz/agave/blob/v2.3.0/platform-tools-sdk/cargo-build-sbf/src/main.rs) |
| SBPF architecture accepted by the candidate validator | Pending successful deployment and invocation |

The host compiler builds off-chain code. The program compiler comes from the SBF platform tools and can differ from the host compiler. The root toolchain pins are due after the spike succeeds.

## Reproduce WP-A0

On a Linux x86-64 host with Agave's installer and Anchor CLI 0.32.1, install the release side by side and select it for this command. The installer stores its configuration under the chosen directory. [Agave 2.3.0 release](https://github.com/anza-xyz/agave/releases/tag/v2.3.0)

```bash
(
    set -e
    spike_tools="${XDG_CACHE_HOME:-$HOME/.cache}/settle463/agave-2.3.0"
    mkdir -p "$spike_tools"
    agave-install-init --no-modify-path --config "$spike_tools/config.yml" \
        --data-dir "$spike_tools/install" 2.3.0
    PATH="$spike_tools/install/active_release/bin:$PATH" bash scripts/phase-a0-spike.sh
)
```

With the candidate tools already on PATH, run `bash scripts/phase-a0-spike.sh` directly from the project root. Cargo, platform-tools and npm packages must be cached or downloadable. The fresh local validator needs ports 19298–19300 and its validator ports available.

The script creates an Anchor project and a disposable local payer in a private temporary directory. It builds the program, funds that payer at validator genesis, deploys the shared object to the loopback RPC, and calls the generated `initialize` instruction at finalized commitment. It prints the scratch path and retains the build outputs and validator log there. Record the tool versions, genesis hash, program ID, deployment signature, and invocation signature in [the phase evidence](../build/phase-a-evidence.md) when this run succeeds.
