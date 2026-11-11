---
name: bezalel-neoforge
description: Design and implement Java 21 mods for Bezalel's NeoForge Minecraft Java 1.21.1 target.
---

# NeoForge on Minecraft 1.21.1

Use official Mojang mappings and NeoForge 21.1.217. Use the `net.neoforged` namespace and `@Mod` with a matching ID. Register content on the mod event bus. Keep dedicated-server-compatible code separated from client rendering and input. Run authoritative gameplay state changes on the server.

Keep the mod ID stable across edits. Use lowercase ASCII names matching `[a-z][a-z0-9_]{1,63}`. Include `src/main/resources/META-INF/neoforge.mods.toml` with concrete versions and compatible Minecraft and NeoForge dependency ranges. Keep resource identifiers and namespaces consistent. In 1.21.1 use singular data paths such as `recipe` and `loot_table`.

Create and edit the full project directly in the workspace, including source, resources, build scripts, dependencies and tests as needed. The optional Bezalel isolated builder consumes src/main/java and src/main/resources with pinned dependencies; use the project build tools when the mod needs a different build configuration.
