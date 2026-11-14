# Runtime mod dependencies

The Windows client resolves local-pack runtime dependencies through `src-tauri/src/dependencies/`. Imports, Studio installation and updates, moving a Studio mod, and the agent's `bezalel_install_mod` tool all use the resolver. `bezalel_dependency_inventory` gives the agent actual installed identities and provenance. The agent workspace uses Codex workspace-write restrictions; pack writes go through the application backend. Existing agent threads are upgraded to a tool-capable thread on their next turn, with recent visible conversation context and the same project files.

On Windows the private Codex App Server explicitly enables the unelevated sandbox and binds its runtime workspace roots to the current draft. A workspace policy without an enabled Windows backend makes Codex reject even benign shell commands and in-workspace patches. PowerShell runs with the sandbox's language restrictions; project file commands and native builds still use the draft boundary. Build caches should stay under the draft. No unrestricted fallback is used.

## User flow

1. Choose a local modpack and import a runtime JAR, install a tested Studio build, or ask the agent to install its workspace JAR.
2. Approve **Search and stage**. This explicitly covers publisher API lookup and bounded candidate downloads for static inspection, not execution or installation.
3. If runtime metadata only supplies a mod ID, the resolver searches and inspects publisher-verified candidate JARs automatically. Names alone never establish identity. A unique compatible primary mod identity is preferred over bundled or compatibility-layer aliases. Multiple matching primary projects stop resolution with an explanation instead of a routine project-choice dropdown. Provider-declared project/release IDs and the official Fabric API project are resolved directly.
4. Review **Install**: all filenames, versions, provider project/release identities, reasons, and owned Studio-mod replacements. Allow approves only that immutable plan. Deny stops the operation.
5. The arrow immediately beside Allow opens Always Allow. Its scope is the exact local pack UUID, target, action and source set. Search/staging and installation are separate grants. Identity choices and replacements always require one-time approval. Settings > Downloads lists and revokes grants, and has an offline resolution switch.

Cards appear inside Studio chat; pack imports show the same cards in pack details. Pending requests survive frontend reloads by re-querying the backend. Requests expire after 30 minutes; process restart cancels pending requests. Repeated/stale answers are rejected. Cancellation drops pending requests and temporary staging. A changed modpack inventory invalidates an approved plan and requires a fresh attempt and approval. Parallel operations on one pack are rejected by the existing process and filesystem locks.

## Compatibility and installation

- Supported targets are the existing Fabric 0.16.14, Forge 52.1.0 and NeoForge 21.1.217 targets for Minecraft 1.21.1 / Java 21.
- Read `fabric.mod.json`, `META-INF/mods.toml`, and `META-INF/neoforge.mods.toml`, manifests, and declared nested runtime JARs. Never run mod classes, Gradle or third-party scripts to inspect dependencies.
- Resolve required dependencies recursively; validate all collected constraints again after closure. Detect duplicate runtime identities, incompatible intersections, cycles, loader/Minecraft/Java constraints and client-only/server-only metadata. Skip absent optional/recommended mods; validate optional mods when present; show soft conflict warnings. The local launcher targets a client environment; server-only dependency edges are excluded there. Graph validation also supports server-side checks.
- Only runtime metadata and provider runtime dependency relationships enter the graph. Maven/Gradle compile dependencies and bundled non-mod libraries are not added to `mods/`.
- Fabric API uses the shared resolver for local packs, including its bundled modules. The former unconditional local download path was removed. Signed shared packs retain the sync protocol's pinned Fabric API artifact contract.
- Modrinth API v2 supplies identity, loader/game-version filtering, dependencies, CDN URLs and SHA-512/SHA-1 hashes. CurseForge API v1 is available when the backend has `BEZALEL_CURSEFORGE_API_KEY`; it honors distribution restrictions and missing download URLs. It never synthesizes a forbidden CDN URL. No API key is exposed to the agent or frontend.
- Download only HTTPS URLs on the selected provider's explicit CDN allowlist, with no redirects; bound sizes/timeouts; verify all available supported publisher hashes. Verify hash-identified imported files against publisher metadata too. Manual local files without a public hash match are explicitly labeled user-provided.
- Keep candidate files in temporary staging until the entire plan is approved. Recheck the pack's file/hash inventory, then install with exclusive file creation. Archive only the unchanged, owned Studio JAR being replaced. Reuse the existing reversible JAR transaction and commit provenance with application state. Ordinary errors roll back new files and restore archived files. Existing files, worlds and unrelated projects are preserved.
- Save root/dependency identities, versions, exact publisher releases and URLs, hashes, runtime requirements, provider relationships and reasons in SQLite. Pack details show installed-file provenance; the agent can query the graph.

## Validation

```powershell
npm run build
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo build --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --tests
# Opt-in real network test, not fixture downloads:
cargo test --manifest-path src-tauri/Cargo.toml live_import_resolves -- --ignored --nocapture
# Local pinned CLI protocol registration, no account/model call:
$env:BEZALEL_CODEX_PATH = '<path to Codex CLI 0.167.0 executable>'
cargo test --manifest-path src-tauri/Cargo.toml dependency_tool_registers_with -- --ignored --nocapture
# Windows filesystem enforcement, no model/account request:
cargo test --manifest-path src-tauri/Cargo.toml windows_workspace_sandbox_allows -- --ignored --nocapture
```

The live example imports a generated metadata test JAR requiring Trinkets. The backend searches Modrinth, the resolver automatically verifies the actual Trinkets project, downloads real Trinkets 3.10.0 and Fabric API 0.116.18+1.21.1 from its CDN, verifies publisher checksums, and installs all three JARs in one isolated pack. The test validates the complete graph and saved relationships. No downloaded mod is executed. The imported root is a metadata fixture, not a gameplay mod; its two downloaded dependencies are actual releases.

Tests cover loader metadata and version predicates, missing/transitive/optional dependencies, cycles, duplicate identities, conflicts and intersecting constraints, environments, bundled mods, permission pause/allow/deny/replay, changed plans, Always Allow/persistence/revocation, simultaneous requests, frontend reload and keyboard/menu behavior, real HTTP failure/partial-body/rate-limit responses, checksum failures, file collisions, Windows exclusive file locks, state-commit failure and rollback.

## Deliberate boundaries and remaining verification

- Shared modpacks are immutable signed releases. Dependency tooling refuses direct mutation of them; their existing submission, candidate, publication and sync workflow remains authoritative. This implementation does not recursively add dependency submissions to shared-pack publication.
- This is a bounded conservative resolver (64 fetched mods, 128 expansion steps, up to 24 compatible release candidates per lookup). It checks intersections but does not globally backtrack across already selected transitive versions. It fails with a conflict instead of replacing unrelated mods or weakening constraints. Unrecognized ordered version formats fail closed. Bundled duplicate identities that a loader might arbitrate also require explicit resolution.
- Optional missing mods are skipped, not automatically installed. Existing conflicting mods require the user to remove/replace them explicitly; only a draft's own unchanged installed JAR is automatically replaceable after one-time approval.
- CurseForge needs a real provider key. Its authenticated API/download path has not been live-tested in this run. Arbitrary GitHub release downloads are not enabled; use a verified official local file for projects outside the supported APIs.
- Offline mode relies on local runtime metadata/installed files. It cannot discover dependency relationships published only on an unavailable provider.
- Exception rollback is tested, including Windows locks. Abrupt power loss/process termination during the final multi-file commit is not a crash-atomic filesystem transaction; archived originals remain in `removed-mods` where applicable. Abandoned staging after a hard crash is not installed or executed.
- The real Codex App Server accepts the dynamic tool registration. A signed-in live probe verified patch-tool writes, shell commands and file writes, and resumed conversations under the Windows sandbox. A separate enforcement probe verified that draft writes succeed and direct writes to a sibling pack directory are denied. A full model-driven dependency installation, native visual acceptance, gameplay startup, and live CurseForge were not exercised. Build and static graph validation do not establish gameplay compatibility.

Provider/loader references: [Modrinth API](https://docs.modrinth.com/api/), [CurseForge API](https://docs.curseforge.com/rest-api/), [Fabric metadata](https://docs.fabricmc.net/develop/loader/fabric-mod-json), [Forge mod files](https://docs.minecraftforge.net/en/1.21.x/gettingstarted/modfiles/), [NeoForge 1.21.1 mod files](https://docs.neoforged.net/docs/1.21.1/gettingstarted/modfiles/).
