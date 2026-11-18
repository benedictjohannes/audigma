# Contributing to Audigma

Thank you for your interest in contributing to `audigma`!

Audigma is an open-source Rust audio engine, CLI, and Linux audio service for high-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD). Inspired by foobar2000's `foo_dsd_processor`, it integrates with PipeWire and ALSA so Linux desktop applications and dedicated music players can output high-rate PCM or 1-bit DSD (Native or DoP via exclusive ALSA) without manual device switching.

Before contributing, please read this document and the referenced specifications.

---

## 1. Project Status & Documentation Philosophy

Audigma is currently in active pre-release development. No crates are published to crates.io yet, meaning internal Rust APIs and configuration options may evolve without backward compatibility guarantees. However, documentation accuracy and external telemetry contracts are strictly guarded:

- **[Design.md](Design.md) is the authoritative audio specification:** It defines platform-neutral audio behavior, mathematical formulas, filter topologies, and reference configurations. A statement belongs in `Design.md` only if it remains true under an alternative implementation that keeps the same output, configuration semantics, and interfaces. Do not add implementation details or speculative features here.
- **Backend-specific behavior lives in [docs/backends/](docs/backends/):** Details specific to sound servers or hardware layers belong in [docs/backends/PipeWire.md](docs/backends/PipeWire.md) and [docs/backends/ALSA.md](docs/backends/ALSA.md).
- **Telemetry contracts in [docs/Telemetry.md](docs/Telemetry.md):** Status fields reported by `audigma status` and named real-time log event names are external interfaces. Renaming or removing them is considered a breaking change.
- **Planned features live in [docs/Roadmap.md](docs/Roadmap.md):** Planned work (e.g. cross-platform ports, new diagnostics) is tracked here and only moves to `Design.md` once built and verified.
- **Implementation reasoning belongs in rustdoc:** Explain *why* a particular implementation approach was chosen directly in rustdoc comments next to the code, rather than bloating `Design.md`.
- **Code cites documentation:** Source code explicitly cites documentation sections where applicable (e.g. `Design.md §4.4` or `docs/backends/ALSA.md §5`). If you renumber or reorganize documentation sections, update all citing code references.

---

## 2. Proposals First

Before opening a pull request for new features, architectural changes, or new configuration keys, **please open a proposal issue first**. Scope and design requirements must be aligned against [Design.md](Design.md) and [docs/Roadmap.md](docs/Roadmap.md) before writing code, helping you avoid wasted effort.

---

## 3. Two-Track Verification Standards

Audigma maintains strict verification standards depending on the subsystems being modified:

### Track A: Audio Engine & Mathematical Code (`audigma-dsp`, `audigma-format`, `tools/design/`)
If you touch filter design, resampling math, CRFB modulators, dithering, or bit-packing:
1. **Measurement Parity or Improvement:** All changes must demonstrate parity or improvement against the quality thresholds defined in `Design.md §8.6` (the audibility and 24-bit transparency anchors, with their per-stage requirements). Note that the $140\text{ dB}$ filter figures in `Design.md §4.2` are design goals; the gated requirements are in §8.6.
2. **Spectral Verification Suite:** Run the automated spectral verification harness:
   ```bash
   cargo run -p audigma-measurements --bin measure_harness
   python tools/measure/measure_spectral.py
   ```
   All checks in generated reports (`docs/measurements/`) must pass with zero failing checks.
3. **Benchmark Before/After Report:** DSP changes can quietly cost real-time headroom, which matters most on weaker hardware. Run the benchmark on the base commit and again on your change, on the same machine with as little else running as you can manage, then include the comparison in your PR:
   ```bash
   uv run tools/benchmark/benchmark.py --tag=before    # on the base commit
   uv run tools/benchmark/benchmark.py --tag=after     # on your branch
   uv run tools/benchmark/benchmark.py --source=after --compare=before
   ```
   The committed `reference` baseline in `docs/benchmarks/` comes from the maintainer's machine, so it is not a fair comparison for your hardware. Paste the comparison summary and your CPU model into the PR description. The gate from `Design.md §8.5` still applies: at least $2\times$ real-time and p99.9 worker time within 70% of the block duration. Changes that do not touch the DSP path need no report.
4. **Filter Table Generation:** Modulator and filter coefficient tables in `crates/audigma-dsp/src/tables.rs` are generated artifacts. Do not edit `tables.rs` by hand. Modify the synthesis scripts in `tools/design/` (`design_src.py`, `design_halfband.py`, `design_crfb.py`), run table export, and format the output:
   ```bash
   python tools/design/export_tables.py
   cargo fmt
   ```

### Track B: Runtime, IO, & Lifecycle (`audigma-engine`, `audigma-io`, `audigma-cli`)
If you modify the feed loop, worker pool, device lifecycle, buffering, or rate switching:
1. **Real Hardware Device Listening Tests Are Required:** Synthetic tests and unit mocks are necessary but not sufficient. Audio pipelines can pass unit tests while introducing audible pops, DAC relay thrashing, or stream relinking glitches on real hardware.
2. **Local Playback Test Fixtures:** We strongly recommend setting up a personal, local (untracked) playback test folder containing reference tracks across format boundaries:
   - PCM 44.1 kHz
   - PCM 96 kHz (cross-rate / cross-family)
   - DSD64 (PCM $\leftrightarrow$ DSD transition)
   Along with a script or player commands to cycle through them while `audigma` is running.
3. **Real-Time Safety Invariants:** In steady state, the audio loop and DSP workers must execute with **zero heap allocations, zero lock contention, and zero I/O**. All threads must flush subnormal floating-point numbers to zero (FTZ/DAZ).

---

## 4. Cross-Platform Contributions

Audigma is designed around pure, portable core crates with platform-specific IO layers:
- Portable crates: `audigma-dsp`, `audigma-format`, `audigma-settings`, `audigma-log`, `audigma-engine`, and `audigma-configurator` core.
- OS-specific crate: `audigma-io` (ALSA, PipeWire, D-Bus device reservation, thread scheduling, the worker pool's futex wake-up).
- Mostly portable: `audigma-cli` has a Linux-only system tray ([Design.md §2](Design.md)).

Windows and macOS support are on the [roadmap](docs/Roadmap.md).

Contributions implementing cross-platform synchronization or new platform backends are warmly welcomed. However, **the requirement for real hardware listening tests is not relaxed for other operating systems**. PRs adding platform backends must include real hardware testing notes, DAC models tested, and behavior reports in the PR description.

---

## 5. Agentic Contributions & AI Guidelines

AI coding assistants (Claude, Codex, Antigravity, Cursor, etc.) are welcomed in this repository! However, to protect repository hygiene:

1. **`AGENTS.md` and scratchpad / local testing folder must Never Be Committed:**
   - Local scratch notes, planning files, and agent instructions are personal to each contributor's machine.
   - Do not cite or link planning files from committed code or docs.
   - `AGENTS.md` is git-ignored. Keep `scratchpad/` out of your commits too (for example with `.git/info/exclude`, or a local pre-commit hook).
2. **Set Up Your Agent Instructions:**
   Copy `AGENTS.template.md` to `AGENTS.md` (which is git-ignored):
   ```bash
   cp AGENTS.template.md AGENTS.md
   ```
   The template includes an interactive directive that prompts your AI agent to interview you about your local DAC, test files, and playback tooling to configure itself.

---

## 6. Development Prerequisites & Building

### Prerequisites (Linux)
- **Rust:** Rust 1.85+ (2024 edition).
- **System Libraries:**
  - ALSA development headers (`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora).
  - PipeWire development headers (`libpipewire-0.3-dev`, `libspa-0.2-dev`).
- **Python (Optional, for DSP tooling):** Python 3.10+ with `numpy`, `scipy`.
- **Bun (Optional, for Web UI development only):** [Bun](https://bun.sh/) 1.1+.

### Building
- **Full Rust Stack & TUI Configurator:**
  ```bash
  cargo build --release
  ```
  *Note:* You do **not** need Bun or Node installed to build Audigma. If the pre-compiled Web UI is not present, `cargo build` automatically embeds a lightweight fallback page. The audio daemon, CLI commands, and the terminal TUI configurator (`audigma configure tui`) work 100% out of the box with `cargo` alone.

- **Building the Web Configurator UI (Optional):**
  If you wish to build or edit the embedded Web UI frontend:
  ```bash
  cd crates/audigma-configurator/web
  bun install --frozen-lockfile
  bun run build
  cd ../../..
  cargo build --release
  ```

---

## 7. Commit & Pull Request Standards

- **Commit Messages:** Follow [Conventional Commits](https://www.conventionalcommits.org/) (e.g. `feat(dsp): ...`, `fix(alsa): ...`, `docs: ...`, `refactor(engine): ...`).
- **Formatting & Lints:**
  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
- **No roadmap references in code:** source comments cite documents that describe shipped behavior (`Design.md`, `docs/backends/`, `docs/Telemetry.md`), never `docs/Roadmap.md` items.
- **Documentation Parity:** If your PR modifies configuration schema, command behavior, or telemetry events, ensure `Design.md`, `docs/backends/`, or `docs/Telemetry.md` are updated in the same commit/PR.
