# Remote Integration Plan: slarti-ssh ↔ slarti-remote

This document outlines a pragmatic plan to integrate the local app (`slarti`) with remote hosts over SSH using:

- `slarti-ssh` (transport: SSH subprocess management, sync/deploy helpers, JSON-over-stdio tunnel)
- `slarti-remote` (agent: runs on target host, exposes discovery/metrics APIs via `slarti-proto`)
- `slarti-proto` (messages: requests/responses, versioning, capabilities)

The goal is to enable host resource/function discovery, with a clean deployment/upgrade story for the remote agent and a clear UX in the Host panel.

---

## Objectives

- On host selection:
  - If agent is present and compatible → connect and start discovery immediately.
  - If agent is missing/outdated → prompt for deployment/upgrade, sync the agent, then connect.
- Persist agent deployment status per host locally (so next selection is fast).
- Keep the design forward-compatible with background metrics and orchestration features.

---

## Components

### slarti-remote (Agent)

- Single binary that runs on the remote host.
- Operates in `--stdio` mode (default) as a JSON-over-stdio service.
- On startup:
  - Emits `HelloAck { agent_version, capabilities }`.
  - Enters request loop: read `Request`, write `Response`.
- Initial capabilities:
  - `sys_info` (os, kernel, arch, uptime)
  - `static_config` (os-release, cpu/mem totals, disks)
  - `services_list` (systemd non-baseline filtered)
  - `containers_list` (docker/podman)
  - `net_listeners` (ss/procfs)
  - `processes_summary` (top CPU/mem talkers; best-effort)
- Future: streaming subscriptions (`watch_metrics`, `watch_services`, etc.)

### slarti-proto (Protocol)

- Add versioning primitives:
  - `VersionInfo { version: String, caps: Vec<Capability> }`
  - `Hello { client_version } → HelloAck { agent_version, caps }`
- Discovery API (phase 1):
  - `SysInfoRequest → SysInfoResponse`
  - `StaticConfigRequest → StaticConfigResponse`
  - `ServicesListRequest → ServicesListResponse { services, baseline_skipped }`
  - `ContainersListRequest → ContainersListResponse { containers }`
  - `NetListenersRequest → NetListenersResponse { listeners }`
  - `ProcessesSummaryRequest → ProcessesSummaryResponse { procs }`
- Optional later:
  - `MetricsRequest { once | sampling } → MetricsResponse` or streaming `MetricsEvent`
- Keep messages additive and versioned; handshake ensures compatibility.

### slarti-ssh (Transport)

- Responsibilities:
  - Run short `ssh -T` commands and return outputs.
  - Synchronize files (prefer `rsync -az`, fallback to `scp`).
  - Launch the agent via `ssh -T "<remote_path>/slarti-remote --stdio"`, return a JSON client handle.
  - Provide utilities:
    - `check_agent(host) → AgentStatus { present, version, path, can_run }`
    - `deploy_agent(host, artifact) → progress + result`
    - `run_agent(host, remote_path) → JsonStdioClient`
- Reconnect is on-demand initially (later: connection pooling / retry policy).

---

## Deployment & Versioning

- Agent version baked into the binary (e.g., `const VERSION: &str`).
- Remote install path:
  - `~/.local/share/slarti/agent/<version>/slarti-remote`
  - Ensure path exists; `chmod 700` dir, `chmod 755` binary.
- Compatibility check:
  1) Fast path: try to run `~/.local/share/slarti/agent/<ver>/slarti-remote --stdio` via `ssh -T`.
  2) Expect `HelloAck` with the same version; if mismatch or failure → prompt to deploy/upgrade.
- Local caching:
  - Keep artifacts at `~/.local/share/slarti/artifacts/slarti-remote-<target>-<version>.tar.gz`
  - `rsync -az` (idempotent), fallback `scp` if needed.
  - Optional remote checksum verification (`sha256sum`); log but don’t block v1.

---

## State & Persistence

- Per-host state file (local):
  - `~/.local/state/slarti/agents/<alias>.json`
  - Suggested fields:
    ```
    {
      "alias": "hostname-or-alias",
      "last_deployed_version": "X.Y.Z",
      "last_deployed_at": "RFC3339",
      "remote_path": "~/.local/share/slarti/agent/X.Y.Z/slarti-remote",
      "remote_checksum": "optional sha256",
      "last_seen_ok": true
    }
    ```
- Source of truth remains the handshake version on each connection; the state speeds up decisions.

---

## UX Flow (Host Panel)

- On alias selection:
  1) Try quick connect (2s timeout):
     - Launch agent from previously known path.
     - If handshake OK → set status “Connected vX” and begin discovery.
  2) If agent missing/incompatible:
     - Show prompt:
       - Title: “Deploy Slarti agent to <alias>?”
       - Body: “This will copy vX (~N MB) to ~/.local/share/slarti/agent/vX and run it over SSH.”
       - Buttons: “Deploy”, “Cancel”.
     - On “Deploy”:
       - Show inline progress (Compress → Upload → Extract/Install → Verify).
       - Persist state (last_deployed_version, etc).
       - Run agent and continue.
  3) On cancel/failure:
     - Show banner: “Not connected (agent missing or incompatible)” with action “Deploy agent”.

- Once connected:
  - Kick off discovery requests concurrently (sys_info, static_config, services_list, containers_list).
  - Render placeholders with results (append when available).
  - For metrics snapshot: a single `MetricsRequest { once: true }` for v1 UI.

---

## Discovery Details (v1)

- `sys_info` / `static_config`:
  - Parse `/etc/os-release`, `/proc/cpuinfo`, `/proc/meminfo`, disk `statvfs`.
- `services_list`:
  - `systemctl list-unit-files` and `systemctl list-units --type=service --no-pager --no-legend`.
  - Filter with distro-specific “baseline” list (shipped with agent).
- `containers_list`:
  - `docker ps --format ...` or `podman ps --format ...` if dockerd/podman present.
- `net_listeners`:
  - `ss -lntup` if available, else parse `/proc/net/tcp*` and `/proc/net/udp*`.
- `processes_summary`:
  - Parse `/proc/*/stat` and `/proc/*/status` for top CPU/mem.

---

## Security & Footprint

- User-level install avoids root requirements.
- Sync via SSH-only tools (no persistent daemons).
- Optional checks:
  - Verify agent checksum, ensure exec perms.
  - Adopt agent signature verification later if needed.

---

## Error Handling

- Transport issues (SSH unreachable, auth failure) → Host panel shows error banner with “Retry” action.
- Agent handshake mismatch:
  - Offer to upgrade; on refusal keep UI actionable (deploy button).
- Discovery command failures:
  - Return partial results with `warnings` array; surface non-fatal errors in a “Diagnostics” subsection.

---

## Milestones

1) Handshake & Protocol
   - Add `Hello/HelloAck`, `VersionInfo`, and capability scaffolding in `slarti-proto`.
   - Implement in `slarti-remote` and the `slarti-ssh` client.

2) Transport & Deploy
   - `check_agent`, `deploy_agent`, `run_agent` in `slarti-ssh`.
   - Local artifact cache (tar.gz per target triple).
   - Basic state persistence in `~/.local/state/slarti/agents`.

3) Discovery v1
   - Implement `sys_info`, `static_config`, `services_list`, `containers_list`.
   - Bind results into the `HostPanel` placeholders.

4) Metrics Snapshot
   - Implement a one-shot `MetricsRequest` → `MetricsResponse` and render in the panel.

5) Subscriptions (Optional Next)
   - Add `watch_metrics` streaming and lightweight polling loop with backoff.

---

## Target Triples & Build

- Start with:
  - `x86_64-unknown-linux-gnu`
  - `aarch64-unknown-linux-gnu`
- Produce statically linked binaries when possible.
- Pack each binary into `slarti-remote-<target>-<version>.tar.gz` with internal layout:
  ```
  slarti-remote/           # root dir inside archive
    bin/slarti-remote      # executable
    baseline/              # optional config e.g., baseline services
    LICENSE
    VERSION
  ```
- Deployment: extract into `~/.local/share/slarti/agent/<version>/` and symlink/move `bin/slarti-remote` to the expected path.

---

## UI Integration Checklist

- HostsPanel:
  - On select alias:
    - Ensure agent (connect or prompt-deploy).
    - Propagate connection status to HostPanel header.
- HostPanel:
  - Header banner:
    - Connected vX | Not connected (Deploy agent) | Connecting…
  - Sections:
    - Identity (alias, hostname, user, proxy, port).
    - Services & Workloads (non-baseline list).
    - Metrics (snapshot).
    - Planning Notes / Tags (user-provided).
- Prompts & Progress:
  - Standard confirmation prompt for deployment.
  - Inline progress UI inside HostPanel (optional v1) or modal progress.

---

## Persistence & Telemetry (Later)

- Record connection success/failure timestamps per host.
- Cache last discovery snapshot per host to show “stale but useful” data on next selection.
- (Optional) Anonymous diagnostics for agent handshake/compat issues.

---

## Summary

This plan adds a robust, incremental path to remote discovery:

- It introduces a versioned, capability-driven agent (`slarti-remote`), a simple JSON protocol (`slarti-proto`), and a transport/deploy layer (`slarti-ssh`).
- It provides a user-friendly deploy/upgrade flow on first contact or version mismatches.
- It cleanly separates concerns so future metrics streaming and orchestration features can build on the same foundation without rework.

Tracking this plan in code:
- Add a feature flag or milestone labels (e.g., `feat(remote/handshake)`, `feat(remote/deploy)`, `feat(remote/discovery)`) to keep PRs small and focused.