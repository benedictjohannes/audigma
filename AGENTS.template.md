# AGENTS.md (Local Agent Instructions)

<!--
================================================================================
BOOTSTRAP DIRECTIVE FOR AI ASSISTANTS:
If this file still contains unconfigured <PLACEHOLDER> tags, STOP IMMEDIATELY.
Do not execute shell commands or edit codebase files yet.
Read this bootstrap directive and interview the user to customize this file first.
================================================================================
-->

> [!IMPORTANT]
> **AGENT DIRECTIVE — SETUP INTERVIEW REQUIRED:**
> If this file was copied from `AGENTS.template.md` and contains `<PLACEHOLDER>` tokens,
> you must pause and guide the developer through local configuration:
>
> 1. **Operating System & Audio Stack:**
>    - Detect the OS (Linux, macOS, Windows).
>    - Inquire about their playback player / automation tool (e.g. `mpd` + `mpc`, `mpv`, `pw-play`, `afplay`, etc.).
>    - If no test fixture folder or playlist reset script exists, **offer to create one** (e.g. a local uncommitted `test-playback/` or `convertTesting/` folder with a platform-appropriate `reset_playlist.sh` or `reset_playlist.ps1` cycling PCM 44.1k, PCM 96k, and DSD64).
> 2. **Target Hardware:**
>    - Inquire about their test DAC model and endpoint (e.g. ALSA device `hw:CARD=...,DEV=0` on Linux, CoreAudio endpoint on macOS, or WASAPI/ASIO device on Windows).
> 3. **Baseline Configuration File:**
>    - Inquire about their primary test configuration file (e.g. `convertTesting/k17.toml`).
> 4. **Update This File:**
>    - Replace all `<PLACEHOLDER>` tags with the concrete details provided by the developer.
>    - Remove this bootstrap directive comment block, then confirm readiness to the developer.

---

## 1. Repository Status & Compatibility

This repository is public on GitHub, but no crates are published to crates.io yet. As such, internal Rust APIs and configuration schemas do not require backwards compatibility.

- **Local-Only Files:** The `scratchpad/` directory and `AGENTS.md` are not git-ignored (enabling easy tracking and staged/unstaged change comparison), but their contents **must never be committed**. The repository's git pre-commit hook (`.git/hooks/pre-commit`) prevents committing or referencing scratchpad files and `AGENTS.md`. Feel free to use `scratchpad/` for personal task plans, research notes, and intermediate files.
- **Benchmarks:** Due to background system processes that can skew execution budgets and real-time percentiles, **always coordinate with the user before running benchmarks** (`tools/benchmark/` or `audigma benchmark`).
- **Process & Audio Coordination:** Coordinate with the user before using terminal commands to check sound server graph links (e.g. PipeWire linking), finding running `audigma` processes, or starting/stopping/restarting `audigma`.

---

## 2. Real Playback Testing Protocol

For real hardware playback tests on `<DAC_MODEL>`:
1. **Inspection:** Coordinate with the user before inspecting sound server links or audigma daemon processes.
2. **Baseline Daemon Command:**
   ```bash
   ./target/release/audigma run -c <BASELINE_CONFIG_PATH>
   ```
3. **Playback Automation:**
   - Before triggering playback, run `<PLAYLIST_RESET_SCRIPT>` to ensure the test queue starts with fixture tracks:
     1. Track #1: PCM 44.1 kHz
     2. Track #2: PCM 96 kHz (cross-rate / cross-family switch)
     3. Track #3: DSD64 (PCM to DSD format transition)
   - Trigger playback using `<PLAYBACK_COMMAND>`.
4. **Relink & Settling Delays:**
   - To prevent sound servers (such as PipeWire) from relinking streams to other output nodes during transitions:
     - Apply a 1-second delay between starting `audigma` and triggering playback.
     - Apply a 1-second delay between stopping playback and stopping/killing `audigma`.
   - Avoid pausing playback if the player holds the stream open (e.g. `mpc pause`), as this causes sound server stream relinking if the audigma sink disappears. Stop playback completely unless explicitly testing stream-pause relink behavior.

---

## 3. Documentation Standards

Audigma follows a strict documentation hierarchy:
- **`Design.md`:** Authoritative platform-neutral audio specification (with formulas where they apply), behavior, and reference configurations. A statement belongs in `Design.md` only if it remains true under an alternative implementation that keeps the same output, configuration semantics, and interfaces.
- **`docs/backends/<Backend>.md`:** Backend-specific behavior and interfaces (e.g. [PipeWire.md](docs/backends/PipeWire.md), [ALSA.md](docs/backends/ALSA.md)).
- **`docs/Telemetry.md`:** Status telemetry fields and named log events (an external interface: renames are breaking).
- **`docs/Roadmap.md`:** Planned behavior; an item moves to `Design.md` when it ships.
- **Implementation Reasoning:** Implementation details belong in rustdoc comments next to the code, rather than growing `Design.md`. When unsure, prefer rustdoc over expanding `Design.md`.
- **Code Citations:** Code explicitly cites documentation sections holding the relevant specification (e.g. `Design.md §4.4` or `docs/backends/ALSA.md §5`). After renumbering or updating sections, update all citing code references.
