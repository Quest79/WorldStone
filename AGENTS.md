# AGENTS.md

How to work in this repo. **What** to build is in [`specs/worldstone-spec.md`](specs/worldstone-spec.md); read it before implementing a feature.

## The prime rule

**The app automates things. Claude does not automate things for the app.**

- Every pipeline step (fetch, build, extract, database, configure, run, accounts, bots) must be performed by WorldStone's own code through its own buttons.
- Never hand-run a step in a terminal and count it as done. If a step can only be completed by hand, the app is broken: fix the app.
- Every control must work. No placeholder buttons, dead nav entries or "coming soon" pages. Ship a control only with the code behind it; a control that can't run now is disabled and says why.
- Show only real state. No fake progress, counts or statuses; unknown is shown as unknown.
- **Playerbots must be built in, enabled and verifiably working** for any emulator that is listed as supported.

## Stack

Tauri 2 · Rust backend · React 19 + TypeScript + Vite · Tailwind CSS 4 · Lucide icons. Windows-first. Everything the server needs runs in Linux containers; the host needs only Git and Docker Desktop.

## Commands

```powershell
npm run desktop                                        # app with hot reload (Rust edits restart it)
npm test                                               # frontend unit tests
cargo test --manifest-path src-tauri/Cargo.toml --lib  # backend tests (use --lib, see below)
npm run build                                          # type-check + production build
```

## Architecture rules

- All process spawning goes through `src-tauri/src/process.rs`. Nothing else calls `Command`.
- Long work is a `Task` (`tasks.rs`): one at a time, events to the UI, cancellable only where safe, secrets redacted from logs. Cancelling **interrupts** the child before killing it (`interrupt.rs`): a killed `docker build` client leaves BuildKit compiling.
- Tauri command handlers are thin; logic lives in service modules with tests.
- Emulator-specific facts live in `src-tauri/recipes/*.toml`, validated on load. No emulator names or flags hard-coded in Rust. Only Vanilla (MaNGOS Zero) is implemented now, but **more emulators will be added**, so build every step generically and per instance: no global singletons, per-instance folders, Compose project names, volumes, credentials and host ports (see spec §6).
- Frontend: only `src/api/client.ts` calls the backend. Components render state from `useWorldstone`; logic that decides a status lives in `src/lib` and is unit-tested.
- Never delete, reset or overwrite user data silently. Stage, then replace; back up before database changes.
- No hard-coded absolute paths; handle paths with spaces.
- Don't store secrets in source or logs.

## Verifying a change

Unit tests are necessary but **not sufficient**. For anything user-visible, run the real desktop app and exercise the real button:

1. Launch with the debug port: `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'; npm run desktop`.
2. Drive the actual webview with `playwright-core` `connectOverCDP('http://127.0.0.1:9222')` (install it in the scratchpad, not the project): click the button, read the UI, take a screenshot. **Never call `page.setViewportSize` or any emulation on it**: while connected it pins the real window's content to a fixed size, so the window looks like it will not resize. To test sizes, resize the real window (`Browser.setWindowBounds` on a CDP session) and measure.
3. Confirm the result **on disk / in Docker**, and that it was the app that produced it.

Report what was verified in the real app and what was only unit-tested.

## Environment gotchas

- **Smart App Control** is enforcing on the dev machine and intermittently blocks freshly built unsigned binaries (`os error 4551`, or `can't find crate` for a proc-macro DLL). Check the `Microsoft-Windows-CodeIntegrity/Operational` log, `cargo clean -p <crate>`, rebuild. Prefer `cargo test --lib`. Never change security settings.
- The dev webview reloads when a watched frontend file changes (docs and `specs/` are excluded in `vite.config.ts`); the UI re-syncs a running task through `current_task`.
- `tauri dev` restarts the app when Rust files change. A leftover `worldstone.exe` locks `target\debug\worldstone.exe` (`os error 5`): stop only that process (never other apps' `node`/`msedgewebview2`), then relaunch.
- Shell is Windows PowerShell 5.1: no `&&`; native-command stderr shows as `NativeCommandError` even on success, so check exit codes.
- The official MaNGOS wiki (getmangos.eu) is behind a Cloudflare bot check. Use the upstream repos (`mangoszero/server`, `mangoszero/database`) as the guide; never try to get past the check.

## Before finishing a task

- Relevant tests pass and the build is clean; say so with the numbers, and say what you did not run.
- The change works in the real app, or say it was not exercised there.
- Update `specs/worldstone-spec.md` (milestone table, decisions) if behavior or scope changed.
