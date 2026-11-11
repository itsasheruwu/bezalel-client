# Bezalel

Bezalel is a Windows launcher and mod studio for Minecraft Java Edition. Create a private modpack, describe a real Java mod to Codex, and share the release after you test it on your server. Bezalel is an independent project under the MIT license.

[Bezalel Client website](https://bezalelclient.com/)

## Run the desktop app

Install Node.js 22, Rust with the Windows MSVC toolchain, Visual Studio C++ Build Tools, and Microsoft Edge WebView2. From this directory, run:

```powershell
npm ci
npm run tauri -- dev
```

`npm run build` checks TypeScript and builds the frontend. `cargo check --manifest-path src-tauri/Cargo.toml` checks the native app. With the release checks complete, `npm run tauri -- build` builds the Windows installer.

## Use the mod studio

1. Open **Create Mod**, choose a pack or standalone project, and create a draft. Existing projects appear in the pack's **Edit mod** list and in Studio's project rail.
2. Install Codex CLI **0.167.0**, connect ChatGPT in Settings, and describe the work. Studio runs the full Codex app-server harness in the project's real workspace, with full file, terminal and network access. The loader instructions supply Minecraft context; they do not restrict tools, dependencies or project files. Threads resume between messages. The app displays tool activity, questions and approval requests, and supports stopping a turn.
3. Use **Edit mod source** to change Java and text resources directly, or **Open project folder** for the whole project. Ask Codex to build with the project's build tools when it needs custom dependencies. **Build mod** remains an optional pinned Podman build with client/server startup checks; its build constraints apply to that runner, not to Codex.
4. Export the JAR or install a verified build into a local pack. Imported third-party JARs need their source project to be edited as source; adding a JAR does not reconstruct its original project.
5. **Delete pack** removes its library entry after confirmation. Personal worlds and installed files remain in the local instance directory, and associated mod projects become standalone. A deleted shared pack stays hidden on this PC; the server and other members are unaffected.

Bezalel prepares its own `bezalel-builder` environment while you work in Studio, after one-time Minecraft EULA acceptance. It does not require changes to the user's default Podman machine. Setup progress appears in the project header and chat stays available. Existing sources are compiled as they are; use chat or Generate source to change them before rebuilding.

The catalog pins Fabric **0.16.14**, Forge **52.1.0**, and NeoForge **21.1.217** on Minecraft **1.21.1** and Java **21**. New targets need their own builder image, skill, and multiplayer verification.

## Play and share

- **Native launch** uses a Minecraft-approved Bezalel Microsoft application ID. Register a public desktop app for personal Microsoft accounts and allow the loopback redirect `http://localhost/auth/callback`. Complete Minecraft's [application review](https://aka.ms/mce-reviewappid), then supply the ID at build time through `BEZALEL_MICROSOFT_CLIENT_ID` or in the Accounts screen. Sign in with an account entitled to Minecraft Java Edition. Forge and NeoForge prepare their pinned publisher installers in isolated game caches.
- **Minecraft Launcher** is available while Microsoft approval is pending or if native setup fails. From a pack, choose **Play in Minecraft Launcher** while signed out of Microsoft in Bezalel, or choose **Use Minecraft Launcher** while signed in. Install the pack’s pinned loader once if it is missing. Bezalel refreshes the launcher, creates or updates the pack’s installation and Game Directory, and opens Java Edition with that pack selected. Click **Play** in Minecraft Launcher. Shared releases update the installation automatically. Sign in there if prompted, and use your pack’s server address to join multiplayer.
- **Private sharing** requires a running [pack service](services/sync/README.md). A configured hosted build lets each tester sign in with a private Bezalel access code and set up a profile. The creator selects the shared option when creating a pack. A friend uses a pack invite and opts into trusting the pack owner once. Local testing still supports operator-issued accounts in **Settings → Accounts**.
- **Publish an update** from a shared pack’s Server tab. Review the submitted sources, prepare a candidate, and export its bundle. Back up the server world, replace the managed mods and configs, restart, and test a join before confirming. Friends sync the published revision the next time they open Bezalel or click Play.

## Invited-alpha checks

Before distributing an installer, run these checks with real services and two entitled Minecraft accounts:

- Complete ChatGPT browser sign-in and an App Server turn. Verify Guided questions, cancellation, thread resume, quota failures, and that draft jobs cannot reach host credentials.
- Complete Microsoft sign-in with Bezalel’s approved ID and join an online-mode server. Exercise revoked and expired sign-ins.
- Prepare each loader image, compile a fixture, verify client and dedicated server startup, and run a generated production JAR on both sides. Exercise the official-launcher setup separately.
- Connect two sharing accounts to Postgres and a private S3 bucket. Test invite and trust revocation, interrupted and tampered downloads, updates while Minecraft is open, concurrent publish attempts, and server-first rollback. Check that saves and personal options survive.

Minecraft assets and the Temurin runtime download from their publishers when needed. Budget several GiB for the desktop toolchain and installed Minecraft, plus separate space for each Podman image.
