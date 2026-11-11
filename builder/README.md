# Local mod builder

Bezalel uses Podman to compile Java and start a test client and dedicated server for Minecraft 1.21.1. The toolchains are Fabric Loader 0.16.14 with Fabric API 0.110.0+1.21.1, Forge 52.1.0, and NeoForge 21.1.217. They use Java 21 and Mojang mappings.

## Prepare a builder on Windows

On Windows 11 x64, Studio prepares a dedicated Bezalel builder in the background after one-time Minecraft EULA acceptance. Chat remains available while the selected loader is prepared. Review the Minecraft EULA when prompted, approve Windows setup if needed, and reopen Bezalel after a requested restart. Bezalel starts an existing compatible builder on later builds.

The following steps are for preparing an image manually during development.

1. Install [Podman for Windows](https://podman.io/docs/installation#windows) with the WSL2 provider. The app manages the `bezalel-builder` machine and its `podman-bezalel-builder` WSL distribution, leaving the default Podman machine untouched. To prepare only the generic environment during development, run `./builder/setup-windows.ps1 -Loader fabric -StateDirectory <app-data>/builder-setup-managed -PrepareEnvironmentOnly`. The machine needs about 6 GiB of available memory and several GiB of free disk per prepared target.
2. Read the [Minecraft EULA](https://www.minecraft.net/eula). If you accept, run the following from the Bezalel repository in PowerShell.

   ```powershell
   ./builder/prepare.ps1 -Loader fabric -AcceptMinecraftEula
   ```

3. Repeat with `forge` or `neoforge` to enable those targets. Keep these images local. The bootstrap downloads game dependencies and tests a trusted fixture. It resolves the Gradle base image to an OCI digest and prints the digest so you can pin it with `-BaseImage` on a later bootstrap.

## Build and review

Drafts supply Java files in `src/main/java` and loader metadata and assets in `src/main/resources`. The runner uses a trusted Gradle template and rejects links, redirected files, unexpected paths, and oversized input. It passes the source snapshot to the container over stdin. Runtime containers use an immutable local image ID, no network, no Windows mounts or credentials, a read-only root, and bounded memory, CPU, process count, time, and output.

Compile returns a JAR. Client and server startup run the same source snapshot in independent loader development containers. Client smoke needs a visible Minecraft window and loaded UI resources. Dedicated-server smoke checks the startup message and a local listening port. These are startup checks. Verify an exported JAR in the target pack and on its server before inviting players.
