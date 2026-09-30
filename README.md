# WorldStone

Windows desktop app that builds and runs World of Warcraft emulator servers for you: it fetches the source, builds it, extracts client data, provisions the database, configures everything and runs it in Docker. Two emulators are supported, each with playerbots built in: MaNGOS Zero (Vanilla 1.12.x) and AzerothCore (Wrath of the Lich King 3.3.5a, with the mod-playerbots fork). A server picker in the sidebar chooses which one every page shows.

**Stack:** Tauri 2 · Rust backend · React + TypeScript + Vite · Tailwind CSS 4 · Lucide icons.

Building, extraction and the database all run inside Linux containers, so the host needs only **Git** and **Docker Desktop** (Linux containers). No Visual Studio, CMake or MySQL on the host.

Product requirements: [`specs/worldstone-spec.md`](specs/worldstone-spec.md). Working rules for contributors and coding agents: [`AGENTS.md`](AGENTS.md).

## Status

| Step | State |
| --- | --- |
| Environment check (Git, Docker, engine, Compose) | Done |
| Download / update source (repos + submodules) | Done |
| Build in a container (with playerbots, verified) | Done |
| Extract client data (your own client: 1.12.x for Vanilla, 3.3.5a for AzerothCore) | Done for Vanilla |
| Provision database (schemas, base data, updates, backup first) | Done |
| Configure and run (servers with playerbots, start/stop/restart) | Done |
| Accounts, connect a client, bot health and settings (Play page) | Done |
| Docker images explained, old builds removable (Maintenance page) | Done |
| Home header: one-click Set up, start/pause/resume/restart/stop, Launch game | Done |
| Service dashboard, database backup and restore, combined update, repair, log viewer, confirmed reset (Maintenance and Logs pages) | Done |
| AzerothCore 3.3.5a with playerbots and the server picker (one server runs at a time) | In progress: see Milestone 9 in the spec |
| Several servers at once, LAN/WAN access, further emulators | Planned |

## Develop

Requires Node.js, Rust (stable, MSVC toolchain), Visual Studio C++ Build Tools and the WebView2 runtime.

```powershell
npm install
npm run desktop      # Vite + native window with hot reload
npm test             # frontend unit tests
cargo test --manifest-path src-tauri/Cargo.toml
npm run build        # type-check + production frontend build
```

`npm run dev` opens the frontend alone in a browser; operations are unavailable there because they need the native backend.

> **Windows Smart App Control** can intermittently block freshly built, unsigned binaries (`os error 4551`, or `can't find crate` for a proc-macro DLL). Rebuilding the affected crate usually clears it (`cargo clean -p <crate>`). Turning Smart App Control off is a system security decision, so WorldStone never does it for you.

## Layout

```
src/                 React frontend (pages, components, hooks, lib) — src/api/client.ts is the only place that calls the backend
src-tauri/src/       Rust backend
  process.rs           the one place processes are spawned (cancel, timeout, line streaming)
  tasks.rs             one operation at a time, events to the UI, secret redaction
  git.rs               clone / fast-forward update with safety rules; fingerprints the source
  builder.rs           generates the Dockerfile from the recipe, builds, verifies the image
  client.rs            checks the game client folder (build read from WoW.exe) and remembers it
  extractor.rs         runs the recipe's extraction phases in a container into a Docker volume; resumable
  database.rs          MariaDB in Docker: schemas, base data, ordered updates, backups, an idempotent ledger
  config.rs            writes the server config files from the built samples; only managed keys change
  settings.rs          per-instance choices (realm id, bot population, scaled to the machine)
  compose.rs           generates the per-instance Docker Compose file
  accounts.rs          create accounts / change passwords through the world server console, confirmed in the database
  bots.rs              playerbot health derived from the database, and the bot settings
  connect.rs           points the game client's realm list file (or one per language folder) at the server (saved copy, restore)
  images.rs            lists the Docker images by server (worldstone/<edition>-<role>, for example worldstone/vanilla-world), shows what uses them, removes old builds
  play.rs              launches the game client (pointing it here first) while the servers run
  runtime.rs           start, stop, pause, resume, restart and status of the database and both servers
  backups.rs           per-schema dump files, verified restore with a safety backup first
  dashboard.rs         per-container state, address, uptime, CPU and memory from Docker
  logview.rs           the launcher's, the containers' and the server's log files, bounded and cleaned
  update.rs            source → rebuild → database scripts → restart in one checked operation
  repair.rs            finds what is wrong and fixes what the app can, never deleting data
  reset.rs             the only deletion: typed confirmation, backup first, lists what is kept
  environment.rs       host dependency checks
  recipe.rs            loads and validates emulator recipes
src-tauri/recipes/   one TOML file per emulator (repos, build image and CMake flags, DB layout, ports)
Instances/<id>/      runtime data (Repos, Data, Config, Logs, Backups, Temp, Docker); git-ignored
```

## Safety rules

- Updates only ever fast-forward: local edits are never reset, and a repository on the wrong branch is left alone.
- A clone is staged in `Temp` and moved into place only when complete. A folder that isn't a Git checkout is reported, never deleted.
- Checkouts keep LF line endings (`core.autocrlf=false`) because they are consumed inside Linux containers.
- Recipe values that reach a path, a command or SQL are validated on load.
- Progress bars show only what tools actually report (git transfer, CMake's own build percentage); otherwise progress is indeterminate.
- A build's Docker image is its record: it is tagged with a fingerprint of the exact source and recipe, so "current" is read from Docker, not from a file that could drift. A finished image is only tagged after the playerbots marker file and `mangosd`/`realmd` have been checked inside it.
- Extracted client data lives in a Docker volume as `current/`, `staging/` and `previous/`; a run writes to `staging/` and replaces `current/` only after every folder is verified, so a failed or cancelled run never touches good data. The client folder is mounted read-only.
- Server configuration keeps every line you edited; only the keys the recipe manages are rewritten. The config folder is mounted read-only into the servers, the database is never published on a host port, and the two servers listen on `127.0.0.1` only.
- Stop is graceful: the world server saves and exits, and a non-zero exit code is reported instead of hidden. Pause freezes the servers (Docker pause; nothing is saved) until Resume.
- You supply your own WoW client. WorldStone never downloads game files.
