---
name: bezalel-fabric
description: Design and implement Java 21 mods for Bezalel's Fabric Minecraft Java 1.21.1 target.
---

# Fabric on Minecraft 1.21.1

Use official Mojang mappings and Fabric Loader 0.16.14. Prefer vanilla APIs. If needed, use Fabric API 0.110.0+1.21.1 and declare it in the mod's dependencies. Use `ModInitializer` for common initialization and `ClientModInitializer` for rendering or client-only behavior. A dedicated server must be able to load common code without resolving client classes. Run authoritative gameplay state changes on the server.

Keep the mod ID stable across edits. Use lowercase ASCII names matching `[a-z][a-z0-9_]{1,63}` and keep the namespace consistent in Java, metadata, assets, and data. Include `src/main/resources/fabric.mod.json` with concrete versions and matching entrypoints. In 1.21.1 use singular data paths such as `recipe` and `loot_table`.

Create and edit the full project directly in the workspace, including source, resources, build scripts, dependencies and tests as needed. The optional Bezalel isolated builder consumes src/main/java and src/main/resources with pinned dependencies; use the project build tools when the mod needs a different build configuration.
