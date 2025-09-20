# Roadmap: slarti

This roadmap lays out the next steps to grow `slarti` from a working demo into a Zed-like (ux, performance) infra manager.

---

## Phase 1 — Local Prototype (current)

- [x] Spawn local shell in a PTY (`portable-pty`) and render bytes into a GPUI window.
- [x] Implement minimal remote agent with `list_dir`.
- [x] Implement SSH transport and demo request.

---

## Phase 2 — Terminal Integration

- [ ] Replace naive text view with `alacritty_terminal::Term` grid.
- [ ] Wire key/mouse events from GPUI into the terminal.
- [ ] Render terminal buffer efficiently (only visible rows).
- [ ] Add resize support (forward window size changes to PTY).

---

## Phase 3 — File Explorer

- [ ] Extend `slarti-remote` with:
  - Recursive `list_dir` batching & pagination.
  - `watch_path` (send file system change events).
- [ ] Render a virtualized tree in GPUI:
  - Expand/collapse directories.
  - Lazy load contents on demand.
- [ ] Add basic file actions: open, save, rename, delete.

---

## Phase 4 — Remote Integration

- [ ] Route terminal sessions through `slarti-ssh` to `slarti-remote`.
- [ ] Support multiple concurrent terminals.
- [ ] Handle dropped SSH connections gracefully (reconnect, notify user).
- [ ] Authentication options: agent forwarding, identity files, config.

---

## Phase 5 — UX & Performance

- [ ] Add tabs/split panes in GPUI.
- [ ] Smooth scrolling, keyboard shortcuts, and context menus.
- [ ] Coalesce FS events and PTY updates into ~60–120 Hz refresh.
- [ ] Benchmark against large directories and long-running PTYs.

---

## Phase 6 — Extras

- [ ] Integrate system metrics panel (CPU, memory, network).
- [ ] Configurable SSH targets (`~/.config/slarti/hosts.toml`).
- [ ] Plugin API for new remote commands.

---

### Stretch Goals

- GPU-accelerated text shaping & ligatures.
- Cross-platform file watching abstraction (Linux/macOS/Windows).
- Replace `ssh` subprocess with pure Rust (`russh`) for tighter control.

---

### Guiding Principle

Keep **UI buttery**, **protocol stable**, and **boundaries clean**:

- GPUI + Alacritty for rendering.
- JSON protocol for communication.
- Remote agent does work, local app stays smooth.
