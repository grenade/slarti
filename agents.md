# Agents Guide

This document defines the **structure** of the `slarti` repository and the **rules** for keeping it tidy, consistent, and purpose-bound.  
Think of it as instructions for contributors (human or AI) to follow.

---

## Repository Structure

```
slarti/
├── Cargo.toml           # Workspace definition
├── crates/
│   ├── slarti/          # Main GPUI application (binary)
│   ├── slarti-proto/    # Shared protocol types (Command/Response, DirEntry, etc.)
│   ├── slarti-remote/   # Remote agent (JSON-over-stdio daemon for dir listings, PTY, file ops)
│   └── slarti-ssh/      # SSH client wrapper to run remote agent over ssh -T
└── README.md
```

- **`slarti/`** — GUI app only. No business logic. Wires GPUI, Alacritty, and calls into other crates.
- **`slarti-proto/`** — Protocol schema only. Must remain dependency-light and serialization-focused.
- **`slarti-remote/`** — Headless agent only. Runs remotely. No UI code. Talks JSON over stdio.
- **`slarti-ssh/`** — Transport only. Responsible for establishing SSH subprocess, piping JSON messages, and returning structured responses.

---

## Rules & Conventions

1. **Purpose-bound crates**  
   - Keep each crate single-purpose.  
   - Do not mix UI, protocol, and transport logic in one place.

2. **Dependency discipline**  
   - Add heavy deps (gpui, alacritty_terminal, portable-pty) **only** in crates that need them.  
   - Keep `slarti-proto` free of UI, async runtimes, and heavy crates.

3. **JSON protocol stability**  
   - All cross-process communication (`slarti-remote` <-> `slarti-ssh` <-> `slarti`) must use types from `slarti-proto`.  
   - Never “inline” custom JSON structs elsewhere.

4. **Naming**  
   - Crates must be prefixed with `slarti-`.  
   - Branches should use conventional names:  
     - `feat/<area>/<short-desc>`  
     - `fix/<area>/<short-desc>`  
     - `chore/<area>/<short-desc>`

5. **Formatting & linting**  
   - Always run `cargo fmt` and `cargo clippy --all-targets --all-features` before merging.

6. **Commits & PRs**  
   - Keep commits atomic and scoped to one concern.  
   - PRs should reference the crate(s) touched in the title (e.g. *feat(slarti-remote): add PTY spawn*).

7. **Tests**  
   - Unit tests live in the same crate as the code.  
   - Integration tests for cross-crate behavior live under `slarti/tests/`.

---

### Summary

- **slarti**: UI only.  
- **slarti-remote**: Agent only.  
- **slarti-ssh**: Transport only.  
- **slarti-proto**: Types only.  

Keep boundaries clear.  
Prefer small, composable crates.  
Respect protocol stability.  
