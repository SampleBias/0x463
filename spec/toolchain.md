# Phase A toolchain spike

The first hosted build used Anchor 0.32.1 and Agave 2.3.0. Its platform Cargo 1.84 rejected the edition-2024 manifest of `cpufeatures 0.3.1`. The tested pair uses Anchor CLI, `anchor-lang`, `anchor-spl` and `@anchor-lang/core` 1.2.2, Agave 4.1.2, platform-tools v1.57 and SBPF v3. [First build](https://github.com/SampleBias/0x463/actions/runs/45418399739) · [Anchor 1.2.2 release notes](https://www.anchor-lang.com/docs/updates/release-notes/1-2-2)

## Pinned workspace tools

The [hosted spike](https://github.com/SampleBias/0x463/actions/runs/45425441171) built a single-instruction program, deployed it and finalized its invocation. The generated project selected host Rust and Cargo **1.89.0**, edition **2021**. Anchor CLI, `anchor-lang`, `anchor-spl` and `@anchor-lang/core` were **1.2.2**. The Solana CLI and validator were **Agave 4.1.2**. The bundled builder was **cargo-build-sbf 4.1.0**. The explicit build options were `--tools-version v1.57 --arch v3`. That platform compiler was **rustc 1.95.0-dev** (`ae660768a`), LLVM 22.1.2. The host and program compilers differ.

The SPL dependency disables its defaults and enables `solana-v3`, `token` and `associated_token`. The first feature selects the Rust SDK family shared with `anchor-lang`'s default. Agave 4.1.2 ran the resulting binary. The root [Rust pin](../rust-toolchain.toml) and [Anchor configuration](../Anchor.toml) record the tested versions. The placeholder localnet address belongs to the disposable spike. Phase D replaces it.

## Phase C SQLite

The Phase C coordinator uses SQLx 0.8.6 and links to native SQLite through `pkg-config`. Install SQLite 3.54.0 or newer with its development library before building the service or CLI. SQLx requires the SQLite build options `SQLITE_ENABLE_COLUMN_METADATA` and `SQLITE_ENABLE_UNLOCK_NOTIFY`. Check the installed version with `pkg-config --modversion sqlite3`. The daemon checks the linked library on an in-memory connection before opening its database. CI builds SQLite 3.54.1 from the upstream archive and checks its SHA3-256. [SQLite releases](https://sqlite.org/download.html) · [WAL advisory](https://rustsec.org/advisories/RUSTSEC-2026-0459.html)

## Local development environment before pinning

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
| First candidate SBF platform tools | Agave 2.3.0 defaults to v1.48. The local platform-tools caches contain v1.51 and v1.52. [Agave 2.3.0 builder source](https://github.com/anza-xyz/agave/blob/v2.3.0/platform-tools-sdk/cargo-build-sbf/src/main.rs) |
| SBPF architecture accepted by the candidate validator | v3 in the hosted run |

The host compiler builds off-chain code. The program compiler comes from the SBF platform tools and can differ from the host compiler.

## Reproduce WP-A0

On a Linux x86-64 host with Agave's installer and Anchor CLI 1.2.2, install the release side by side and select it for this command. The installer stores its configuration under the chosen directory. [Agave 4.1.2 release](https://github.com/anza-xyz/agave/releases/tag/v4.1.2)

```bash
(
    set -e
    spike_tools="${XDG_CACHE_HOME:-$HOME/.cache}/settle463/agave-4.1.2"
    mkdir -p "$spike_tools"
    agave-install-init --no-modify-path --config "$spike_tools/config.yml" \
        --data-dir "$spike_tools/install" 4.1.2
    PATH="$spike_tools/install/active_release/bin:$PATH" bash scripts/phase-a0-spike.sh
)
```

With the candidate tools already on PATH, run `bash scripts/phase-a0-spike.sh` directly from the project root. Cargo, platform-tools and npm packages must be cached or downloadable. The fresh local validator needs ports 19298–19300 and its validator ports available.

The script creates an Anchor project and a disposable local payer in a private temporary directory. It builds the program, funds that payer at validator genesis, deploys the shared object to the loopback RPC, and calls the one-instruction program’s `initialize` entry point at finalized commitment. It prints the scratch path and retains the build outputs and validator log there. The [phase evidence](../build/phase-a-evidence.md) records the genesis hash, program ID and both finalized signatures from the passing run.
