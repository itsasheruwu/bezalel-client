# Changelog

## Commit changelog format

Agents and contributors must use this format for commit messages and entries in this file:

```text
+ Added a short description of the main change

+ Added a concrete feature or behavior.
= Fixed a concrete problem and describe the resulting behavior.
- Removed an obsolete feature, behavior, or duplicate implementation.

Validation: List the checks actually run and their results.
```

- Start the subject with `+ Added`, `= Fixed`, or `- Removed`, choosing the main purpose of the commit. Keep it concise, preferably within 72 characters.
- Use the same prefixes for individual changes in the body. Group additions, fixes, and removals in that order, and omit groups with no changes.
- Write plain, specific descriptions of the final behavior. Do not include conversational history, abandoned attempts, placeholder entries, or claims beyond the evidence.
- Update this file in the same commit when behavior changes. Put new entries first under the current date, consolidating related changes into readable lines.
- Record validation separately. Clearly distinguish automated checks from live provider, desktop, and gameplay verification. Never invent validation for historical commits.
- Apply the format to future commits. Rewriting previously published history requires an explicit user request; this convention alone does not authorize it.

## 2026-11-14 - Dependencies and chat interactions

+ Added recursive runtime mod dependency resolution for local Fabric, Forge, and NeoForge packs, with installed-mod inventory and compatibility checks.
+ Added Modrinth and CurseForge API integration, verified staging downloads, safe installation, rollback, and saved dependency provenance.
+ Added backend-enforced dependency approval cards, scoped Always Allow permissions, review and revocation in Settings, and offline resolution.
+ Added automatic chat scrolling, a floating jump-to-latest control, custom slim scrollbars, and contextual actions for conversations, modpacks, mods, text, code, and links.
+ Added connected context-menu surfaces and sliding hover highlights for both sidebars, context menus, and model selection.
+ Added dependency, permission, download failure, rollback, scrolling, menu, and highlight tests, plus agent commit instructions.
= Fixed Windows agent workspace permissions so project commands and patches work while modpack installation stays behind the application tools.
= Fixed dependency cards appearing before the relevant chat activity, verbose approval layouts, and inconsistent button colors.
= Fixed highlight alignment while the model selector scales open and disabled options receiving hover highlights.
- Removed the separate unconditional local Fabric API download path in favor of the shared dependency resolver.
- Removed routine dependency project-choice dropdowns in favor of automatic verified identity matching; ambiguous matches stop with an explanation.
- Removed default browser context-menu commands from app surfaces in favor of relevant application actions.

Validation: 38 UI tests and 42 Rust tests passed; six opt-in Rust tests were skipped. TypeScript/Vite production build, Rust check, formatting, and staged whitespace checks passed. Live provider, signed-in agent, desktop animation, and gameplay checks were not rerun for this commit.

See [runtime dependency documentation](docs/mod-dependencies.md) for provider requirements, validation commands, and remaining limits.

## 2026-11-11 - Studio context and installed mod controls

+ Added persistent thread context usage and an animated, connected context panel.
+ Added installation confirmation, compatible modpack moves, and safe installed mod removal.
+ Added chat history separators for moved and removed mods.
+ Added pack mod actions and refined tab navigation.

## 2026-11-11 - Initial public release

+ Added the Windows Bezalel Client with Fabric, Forge, and NeoForge targets.
+ Added local modpack management, Minecraft Launcher handoff, and the mod development Studio.
+ Added authenticated pack sharing, server release workflows, and isolated mod builds.
