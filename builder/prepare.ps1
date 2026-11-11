[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('fabric', 'forge', 'neoforge')]
    [string] $Loader,

    [switch] $AcceptMinecraftEula,

    [ValidatePattern('^docker\.io/library/gradle@sha256:[a-f0-9]{64}$')]
    [string] $BaseImage
)

$ErrorActionPreference = 'Stop'
$connection = 'bezalel-builder'
$imageRevision = if ($Loader -eq 'forge') { 'v2' } else { 'v1' }
$imageTag = "localhost/bezalel-builder:$Loader-1.21.1-$imageRevision"

if (-not $AcceptMinecraftEula) {
    throw 'Read https://www.minecraft.net/eula, then rerun with -AcceptMinecraftEula if you accept the Minecraft EULA.'
}

Get-Command podman -ErrorAction Stop | Out-Null
$connections = podman system connection list --format json | ConvertFrom-Json
$localConnection = $connections | Where-Object {
    $_.Name -eq $connection -and $_.IsMachine -eq $true -and
        $_.URI -match '^ssh://[^@]+@(127\.0\.0\.1|\[::1\]):[0-9]+/'
}
if (-not $localConnection) {
    throw 'Initialize the local rootless Podman WSL2 machine before preparing a builder.'
}

$security = podman --connection $connection info --format json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $security.host.os -ne 'linux' -or $security.host.cgroupVersion -ne 'v2' -or
    -not $security.host.security.rootless -or -not $security.host.security.seccompEnabled) {
    throw 'The builder requires a running local rootless Linux Podman engine with cgroups v2 and seccomp.'
}

if (-not $BaseImage) {
    $tag = 'docker.io/library/gradle:8.12.1-jdk21'
    podman --connection $connection pull $tag
    if ($LASTEXITCODE -ne 0) { throw "Could not pull $tag." }
    $digest = podman --connection $connection image inspect --format '{{.Digest}}' $tag
    if ($LASTEXITCODE -ne 0 -or $digest -notmatch '^sha256:[a-f0-9]{64}$') {
        throw "Could not resolve an immutable digest for $tag."
    }
    $BaseImage = "docker.io/library/gradle@$digest"
}

Write-Host "Preparing $imageTag from $BaseImage with the trusted $Loader fixture."
podman --connection $connection build --pull=missing --file (Join-Path $PSScriptRoot 'Containerfile') --tag $imageTag `
    --build-arg "BUILDER_BASE=$BaseImage" --build-arg "LOADER=$Loader" --build-arg 'ACCEPT_MINECRAFT_EULA=accepted' $PSScriptRoot
if ($LASTEXITCODE -ne 0) { throw "Preparing $imageTag failed." }

podman --connection $connection image inspect --format '{{.Id}}' $imageTag
if ($LASTEXITCODE -ne 0) { throw "The prepared image could not be inspected." }
