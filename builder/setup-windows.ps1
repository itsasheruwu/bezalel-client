#Requires -Version 5.1
[CmdletBinding()]
param(
    [string] $Loader,
    [string] $MinecraftVersion = '1.21.1',
    [string] $StateDirectory,
    [switch] $AcceptMinecraftEula,
    [switch] $PrepareEnvironmentOnly
)

$ErrorActionPreference = 'Stop'
$machine = 'bezalel-builder'
$distribution = 'podman-bezalel-builder'
$baseImage = 'docker.io/library/gradle@sha256:f8e34f2afdf9f9314c718cfcc327f865721b7abad681e9aea2e00d66b6ee1522'
$installerUrl = 'https://github.com/podman-container-tools/podman/releases/download/v6.1.4/podman-installer-windows-amd64.msi'
$installerHash = '4a05240054cb990629cbe098db090c5c7848c44162c11910cabb45ca75a40d53'
$wsl = Join-Path ([Environment]::SystemDirectory) 'wsl.exe'
$stageCode = 11
$stageMessage = 'Choose a supported loader for Minecraft 1.21.1 before preparing a builder.'

function Enter-Stage([string] $Name, [int] $Code, [string] $Message) {
    $script:stageCode = $Code
    $script:stageMessage = $Message
    [Console]::WriteLine("BEZALEL_SETUP $Name")
}

function Stop-Setup([int] $Code, [string] $Message) {
    $script:stageCode = $Code
    $script:stageMessage = $Message
    throw 'Builder setup stopped.'
}

function Confirm-SetupConsent {
    if (-not $AcceptMinecraftEula -and -not $PrepareEnvironmentOnly) {
        Stop-Setup 10 'Accept the Minecraft EULA in Bezalel before installing or preparing a builder.'
    }
}

function Invoke-Native([string] $Exe, [string[]] $Arguments) {
    # Windows PowerShell can turn native stderr into ErrorRecord objects.
    $ErrorActionPreference = 'Continue'
    $PSNativeCommandUseErrorActionPreference = $false
    $lines = @(& $Exe @Arguments 2>&1)
    $code = $LASTEXITCODE
    [pscustomobject] @{
        Code = $code
        Output = ((($lines | Where-Object { $_ -isnot [Management.Automation.ErrorRecord] }) -join "`n") -replace '\x00', '').Trim()
        Diagnostic = (($lines -join "`n") -replace '\x00', '').Trim()
    }
}

function Get-WslVersion {
    $result = Invoke-Native $wsl @('--version')
    if ($result.Code -eq 0 -and $result.Output -match '\b(\d+\.\d+\.\d+(?:\.\d+)?)\b') {
        return [version] $Matches[1]
    }
}

function Confirm-Wsl {
    if (-not (Test-Path -LiteralPath $wsl -PathType Leaf)) {
        Stop-Setup 20 'Windows could not find its WSL installer. Update Windows, then retry builder setup.'
    }
    $version = Get-WslVersion
    $status = Invoke-Native $wsl @('--status')
    $missingComponent = $status.Diagnostic -match '(?i)WSL_E_(WSL_OPTIONAL_COMPONENT_REQUIRED|VIRTUAL_MACHINE_PLATFORM_REQUIRED|KERNEL_COMPONENT_MISSING|WSL_NOT_INSTALLED)|0x8007019e|0x8004032d|0x80040321'
    if ($version -and $status.Code -eq 0 -and -not $missingComponent) { return $version }
    if ($version -and -not $missingComponent) {
        Stop-Setup 20 'WSL is installed but could not report its status. Check Windows virtualization and WSL, then retry builder setup.'
    }

    Confirm-SetupConsent
    Enter-Stage 'installing-wsl' 20 'Windows could not install WSL. Allow the Windows administrator prompt and check that virtualization is enabled.'
    $install = Start-Process -FilePath $wsl -ArgumentList @('--install', '--no-distribution', '--web-download') `
        -Verb RunAs -WindowStyle Hidden -PassThru -Wait
    if ($install.ExitCode -in @(1641, 3010)) {
        Stop-Setup 21 'Restart Windows to finish installing WSL, then reopen Bezalel and retry.'
    }
    if ($install.ExitCode -ne 0) { Stop-Setup 20 $script:stageMessage }

    $version = Get-WslVersion
    $status = Invoke-Native $wsl @('--status')
    if (-not $version -or $status.Code -ne 0) {
        Stop-Setup 21 'Restart Windows to finish installing WSL, then reopen Bezalel and retry.'
    }
    return $version
}

function Find-Podman {
    $local = [Environment]::GetFolderPath('LocalApplicationData')
    $programs = [Environment]::GetFolderPath('ProgramFiles')
    $paths = @(
        (Join-Path $local 'Programs\Podman\podman.exe')
        (Join-Path $local 'Programs\RedHat\Podman\podman.exe')
        (Join-Path $programs 'Podman\podman.exe')
        (Join-Path $programs 'RedHat\Podman\podman.exe')
        (Get-Command podman.exe -CommandType Application -All -ErrorAction SilentlyContinue |
            Select-Object -ExpandProperty Source)
    )
    $found = $false
    foreach ($candidate in $paths) {
        if ($candidate -and [IO.Path]::IsPathRooted($candidate) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            $found = $true
            $result = Invoke-Native $candidate @('--version')
            if ($result.Code -eq 0 -and $result.Output -match '^podman version \d+\.\d+\.\d+') {
                return [pscustomobject] @{ Path = $candidate; Version = $result.Output }
            }
        }
    }
    if ($found) {
        Stop-Setup 30 'An existing Podman installation could not start. Repair that installation, then retry Bezalel setup.'
    }
}

function Install-Podman {
    Enter-Stage 'installing-podman' 30 'Podman setup failed. Check your connection and available disk space, then retry.'
    $tempDirectory = Join-Path ([IO.Path]::GetTempPath()) ("bezalel-podman-" + [Guid]::NewGuid().ToString('N'))
    $msi = Join-Path $tempDirectory 'podman.msi'
    try {
        New-Item -Path $tempDirectory -ItemType Directory | Out-Null
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -Uri $installerUrl -UseBasicParsing -OutFile $msi -TimeoutSec 180
        if ((Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash -ne $installerHash) {
            Stop-Setup 30 'The Podman installer did not match the published checksum. Retry to download a fresh copy.'
        }
        $msiexec = Join-Path ([Environment]::SystemDirectory) 'msiexec.exe'
        $install = Start-Process -FilePath $msiexec -ArgumentList @('/i', ('"' + $msi + '"'), '/quiet', '/norestart',
            'ALLUSERS=2', 'MSIINSTALLPERUSER=1', 'MACHINE_PROVIDER=wsl') -WindowStyle Hidden -PassThru -Wait
        if ($install.ExitCode -in @(1641, 3010)) {
            Stop-Setup 21 'Restart Windows to finish installing Podman, then reopen Bezalel and retry.'
        }
        if ($install.ExitCode -ne 0) { Stop-Setup 30 $script:stageMessage }
    }
    finally {
        Remove-Item -LiteralPath $msi -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $tempDirectory -ErrorAction SilentlyContinue
    }
}

function Read-Json($Result) {
    if ($Result.Code -ne 0) { throw 'A Podman query failed.' }
    ConvertFrom-Json -InputObject $Result.Output
}

function Get-WslRegistration {
    $registry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss'
    Get-ChildItem -LiteralPath $registry -ErrorAction SilentlyContinue |
        Get-ItemProperty | Where-Object { $_.DistributionName -eq $distribution }
}

function Test-OwnerMarker([string] $Path, [string] $Id) {
    $Id -match '^\{[0-9a-f-]{36}\}$' -and
        (Test-Path -LiteralPath $Path -PathType Leaf) -and
        [IO.File]::ReadAllText($Path).Trim() -eq $Id
}

function Save-OwnerMarker([string] $Path, [string] $Id) {
    New-Item -Path $StateDirectory -ItemType Directory -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Id)
}

function Confirm-LocalConnection {
    $machineInfo = @(Read-Json (Invoke-Native $podman @('machine', 'inspect', $machine)))
    if ($machineInfo.Count -ne 1 -or $machineInfo[0].Rootful -ne $false) {
        Stop-Setup 41 'The Bezalel machine is configured for rootful operation. Bezalel requires a rootless WSL 2 machine.'
    }
    $ssh = $machineInfo[0].SSHConfig
    $connections = Read-Json (Invoke-Native $podman @('system', 'connection', 'list', '--format', 'json'))
    $local = @($connections | Where-Object {
        $_.Name -eq $machine -and $_.IsMachine -eq $true -and
            $_.URI -match '^ssh://[^@/]+@(127\.0\.0\.1|\[::1\]):[0-9]+/run/user/[0-9]+/podman/podman\.sock$' -and
            ([Uri] $_.URI).Port -eq $ssh.Port -and
            ([Uri] $_.URI).UserInfo -eq $ssh.RemoteUsername
    })
    if ($local.Count -ne 1) {
        Stop-Setup 41 'Bezalel needs the managed local rootless Podman machine. Review the Bezalel build environment and its connection in Podman.'
    }
}

function Get-EngineInfo {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $podman
    $start.Arguments = "--connection $machine info --format json"
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(5000)) {
            if (-not $process.HasExited) { $process.Kill() }
            return
        }
        if ($process.ExitCode -eq 0 -and $stdout.Wait(500)) {
            return (ConvertFrom-Json -InputObject $stdout.Result).host
        }
    }
    finally {
        $process.Dispose()
    }
}

function Wait-EngineInfo {
    for ($attempt = 0; $attempt -lt 8; $attempt++) {
        $info = Get-EngineInfo
        if ($info) { return $info }
        Start-Sleep -Seconds 2
    }
}

function Test-IsolationControllers($Info) {
    $Info.os -eq 'linux' -and $Info.cgroupVersion -eq 'v2' -and $Info.security.rootless -eq $true -and
        $Info.security.seccompEnabled -eq $true -and
        'memory' -in $Info.cgroupControllers -and 'cpu' -in $Info.cgroupControllers -and 'pids' -in $Info.cgroupControllers
}

function Invoke-GuestScript([string] $Script) {
    # Base64 keeps Windows PowerShell's native argument quoting out of the guest script.
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($Script -replace "`r`n", "`n")))
    Invoke-Native $wsl @('--distribution', $distribution, '--user', 'root', '--exec', '/bin/bash', '-c',
        "set -o pipefail; echo $encoded | base64 -d | /bin/bash")
}

function Start-NativeMachine {
    Invoke-Native $wsl @('--distribution', $distribution, '--user', 'root', '--exec', '/root/bootstrap') | Out-Null
}

function Test-GuestRunning {
    $running = Invoke-Native $wsl @('--list', '--running', '--quiet')
    ($running.Output -split '\r?\n') -contains $distribution
}

function Stop-GuestForRestart {
    if (Test-GuestRunning) {
        if ((Invoke-Native $wsl @('--terminate', $distribution)).Code -ne 0) { Stop-Setup 42 $script:stageMessage }
    }
}

function Confirm-NoRunningContainers {
    $running = Invoke-Native $podman @('--connection', $machine, 'ps', '--quiet')
    if ($running.Code -ne 0 -or $running.Output) {
        Stop-Setup 42 'Stop the running containers on the Bezalel Podman machine, then retry setup.'
    }
}

function Restart-ManagedNativeMachine {
    Confirm-NoRunningContainers
    Invoke-Native $podman @('machine', 'stop', $machine) | Out-Null
    Stop-GuestForRestart
    Start-NativeMachine
    Confirm-LocalConnection
    Wait-EngineInfo
}

function Repair-ManagedMachine([string] $Id, [string] $NativeMarker) {
    Enter-Stage 'repairing-cgroups' 42 'The Podman machine could not enable the CPU, memory, and process limits required by the builder.'
    Confirm-NoRunningContainers
    $repair = @'
set -eu
backup=/var/lib/bezalel-native-systemd
test ! -e "$backup"
test -f /etc/wsl.conf && test ! -L /etc/wsl.conf
test -f /root/bootstrap && test ! -L /root/bootstrap
test "$(cat /proc/1/comm)" != systemd
if grep -Eq '^[[:space:]]*\[boot\]' /etc/wsl.conf; then exit 1; fi
install -d -m 0700 "$backup"
cp -p /etc/wsl.conf "$backup/wsl.conf"
cp -p /root/bootstrap "$backup/bootstrap"
restore_on_error() {
    cp -p "$backup/wsl.conf" /etc/wsl.conf
    cp -p "$backup/bootstrap" /root/bootstrap
}
trap restore_on_error ERR
printf '\n[boot]\nsystemd=true\n' >> /etc/wsl.conf
cat > /root/bootstrap <<'EOF'
#!/bin/bash
if [ "$(cat /proc/1/comm)" = systemd ]; then
    timeout 60 systemctl is-system-running --wait >/dev/null 2>&1 || true
    if ! pgrep -fx 'sleep infinity' >/dev/null; then
        nohup sleep infinity </dev/null >/dev/null 2>&1 &
    fi
    sleep 0.1
    exit 0
fi
exec /var/lib/bezalel-native-systemd/bootstrap "$@"
EOF
trap - ERR
'@
    if ((Invoke-GuestScript $repair).Code -ne 0) { Stop-Setup 42 $script:stageMessage }
    Save-OwnerMarker $NativeMarker $Id

    $info = Restart-ManagedNativeMachine
    if (Test-IsolationControllers $info) { return $info }

    # Restore the original guest files if native systemd cannot delegate the controllers.
    $restore = @'
set -eu
backup=/var/lib/bezalel-native-systemd
test -f "$backup/wsl.conf" && test -f "$backup/bootstrap"
cp -p "$backup/wsl.conf" /etc/wsl.conf
cp -p "$backup/bootstrap" /root/bootstrap
mv "$backup" "$backup.failed"
'@
    if ((Invoke-GuestScript $restore).Code -eq 0) {
        Remove-Item -LiteralPath $NativeMarker -Force
        Stop-GuestForRestart
    }
    Stop-Setup 42 $script:stageMessage
}

function Get-BuilderImage([string] $Tag) {
    $result = Invoke-Native $podman @('--connection', $machine, 'image', 'inspect', $Tag)
    if ($result.Code -eq 0) {
        $images = @(Read-Json $result)
        if ($images.Count -eq 1 -and $images[0].Labels.'org.bezalel.minecraft-eula' -eq 'accepted' -and
            $images[0].Id -match '^(?:sha256:)?[a-f0-9]{64}$') {
            return $images[0].Id
        }
    }
}

function Confirm-ResourceLimits([string] $Image) {
    $check = Invoke-Native $podman @('--connection', $machine, 'run', '--rm', '--pull=never', '--network=none',
        '--http-proxy=false', '--read-only', '--cap-drop=all', '--security-opt=no-new-privileges', '--user=1000:1000',
        '--cgroupns=private', '--pids-limit=256', '--memory=6g', '--memory-swap=6g', '--cpus=2', '--image-volume=ignore',
        '--log-driver=none', '--timeout=20', '--entrypoint', '/bin/sh', $Image, '-c',
        'cat /sys/fs/cgroup/memory.max /sys/fs/cgroup/memory.swap.max /sys/fs/cgroup/cpu.max /sys/fs/cgroup/pids.max')
    if ($check.Code -ne 0 -or $check.Output -notmatch '^6442450944\s+0\s+200000\s+100000\s+256$') {
        Stop-Setup 42 'Podman could not enforce the expected CPU, memory, swap, and process limits. The builder requires those limits.'
    }
}

$mutex = $null
$locked = $false
$exitCode = 0
try {
    Enter-Stage 'checking' 11 $stageMessage
    if ($Loader -notin @('fabric', 'forge', 'neoforge') -or $MinecraftVersion -ne '1.21.1' -or -not $StateDirectory) {
        Stop-Setup 11 $stageMessage
    }
    if (-not [Environment]::Is64BitProcess -or
        [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
        Stop-Setup 12 'Automatic builder setup requires Windows x64.'
    }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $mutex = [Threading.Mutex]::new($false, "Local\Bezalel.BuilderSetup.$identity")
    try { $locked = $mutex.WaitOne(0) }
    catch [Threading.AbandonedMutexException] { $locked = $true }
    if (-not $locked) { Stop-Setup 60 'A Bezalel builder setup is already running.' }

    foreach ($variable in @('CONTAINER_HOST', 'CONTAINER_CONNECTION', 'CONTAINER_SSHKEY', 'DOCKER_HOST', 'CONTAINERS_CONF',
        'CONTAINERS_CONF_OVERRIDE', 'CONTAINERS_CONF_MODULES', 'CONTAINERS_STORAGE_CONF', 'CONTAINERS_REGISTRIES_CONF',
        'PODMAN_CONNECTIONS_CONF', 'CONTAINERS_MACHINE_PROVIDER')) {
        Remove-Item -LiteralPath "Env:$variable" -ErrorAction SilentlyContinue
    }
    $env:CONTAINERS_MACHINE_PROVIDER = 'wsl'
    $env:WSL_UTF8 = '1'
    $wslVersion = Confirm-Wsl
    $installation = Find-Podman
    if (-not $installation) {
        Confirm-SetupConsent
        Install-Podman
        $installation = Find-Podman
    }
    if (-not $installation) { Stop-Setup 30 'Bezalel could not find Podman after setup. Reopen Bezalel or install Podman for the current Windows user.' }
    $podman = $installation.Path
    $env:Path = "$(Split-Path -Parent $podman);$env:Path"

    $ownerMarker = Join-Path $StateDirectory 'machine-id'
    $nativeMarker = Join-Path $StateDirectory 'native-systemd-machine-id'
    $initializingMarker = Join-Path $StateDirectory 'initializing-machine'
    $stageCode = 40
    $stageMessage = 'Podman could not read the Bezalel build environment. Check Podman and WSL, then retry setup.'
    $machines = Read-Json (Invoke-Native $podman @('machine', 'list', '--format', 'json'))
    $default = @($machines | Where-Object { $_.Name -eq $machine })
    $registration = @(Get-WslRegistration)
    if ($default.Count -eq 0 -and $registration.Count -eq 0) {
        Confirm-SetupConsent
        Enter-Stage 'initializing-machine' 40 'Podman could not initialize a local rootless WSL 2 machine. Check disk space and WSL, then retry.'
        Save-OwnerMarker $initializingMarker $machine
        $priorConnections = Read-Json (Invoke-Native $podman @('system', 'connection', 'list', '--format', 'json'))
        $priorDefault = @($priorConnections | Where-Object { $_.Default -eq $true } | Select-Object -ExpandProperty Name -First 1)
        if ((Invoke-Native $podman @('machine', 'init', '--rootful=false', '--cpus', '2', '--memory', '6144', '--disk-size', '30', $machine)).Code -ne 0) {
            Stop-Setup 40 $stageMessage
        }
        if ($priorDefault.Count -eq 1) {
            Invoke-Native $podman @('system', 'connection', 'default', $priorDefault[0]) | Out-Null
        }
        $registration = @(Get-WslRegistration)
        if ($registration.Count -ne 1 -or $registration[0].Version -ne 2 -or
            $registration[0].PSChildName -notmatch '^\{[0-9a-f-]{36}\}$') {
            Stop-Setup 40 $stageMessage
        }
        Save-OwnerMarker $ownerMarker $registration[0].PSChildName
    }
    elseif ($default.Count -ne 1 -or $default[0].VMType -ne 'wsl' -or $registration.Count -ne 1 -or
        $registration[0].Version -ne 2) {
        Stop-Setup 41 'A different machine already uses the Bezalel name. Bezalel requires a local rootless WSL 2 build environment.'
    }

    Confirm-LocalConnection
    $registrationId = $registration[0].PSChildName
    if (Test-OwnerMarker $initializingMarker $machine) {
        Save-OwnerMarker $ownerMarker $registrationId
        Remove-Item -LiteralPath $initializingMarker -Force
    }
    $managed = Test-OwnerMarker $ownerMarker $registrationId
    $native = $managed -and (Test-OwnerMarker $nativeMarker $registrationId)
    $info = Get-EngineInfo
    if (-not $info -and (Test-GuestRunning)) { $info = Wait-EngineInfo }
    if (-not $info) {
        Enter-Stage 'starting-machine' 40 'The Bezalel machine did not start. Open Podman or check WSL, then retry.'
        if ($native) { Start-NativeMachine }
        elseif (-not (Test-GuestRunning)) {
            Invoke-Native $podman @('machine', 'start', '--update-connection=false', $machine) | Out-Null
        }
        else { Stop-Setup 40 $stageMessage }
        Confirm-LocalConnection
        $info = Wait-EngineInfo
    }
    if (-not $info) { Stop-Setup 40 $stageMessage }
    if ($info.os -ne 'linux' -or $info.cgroupVersion -ne 'v2' -or
        $info.security.rootless -ne $true -or $info.security.seccompEnabled -ne $true) {
        Stop-Setup 41 'The Bezalel machine must use rootless Linux, cgroups v2, and seccomp.'
    }
    if (-not (Test-IsolationControllers $info)) {
        if ($native) {
            Confirm-SetupConsent
            Enter-Stage 'repairing-cgroups' 42 'The Bezalel Podman machine could not finish enabling resource limits.'
            $info = Restart-ManagedNativeMachine
            if (-not (Test-IsolationControllers $info)) { Stop-Setup 42 $stageMessage }
        }
        elseif ($managed -and $wslVersion.Major -ge 3 -and
            $installation.Version -eq 'podman version 6.1.4' -and $info.cgroupManager -eq 'cgroupfs') {
            Confirm-SetupConsent
            $info = Repair-ManagedMachine $registrationId $nativeMarker
        }
        else {
            Stop-Setup 42 'The Bezalel machine does not expose CPU, memory, and process limits. Use a compatible rootless Podman WSL machine, then retry.'
        }
    }
    # Preparing the generic Linux tools does not run or download Minecraft.
    # Minecraft content and fixture startup still require explicit EULA consent.
    if ($PrepareEnvironmentOnly) {
        [Console]::WriteLine('BEZALEL_SETUP environment-ready')
        return
    }
    $revision = if ($Loader -eq 'forge') { 'v2' } else { 'v1' }
    $tag = "localhost/bezalel-builder:$Loader-1.21.1-$revision"
    $imageId = Get-BuilderImage $tag
    if (-not $imageId) {
        Confirm-SetupConsent
        $base = Invoke-Native $podman @('--connection', $machine, 'image', 'exists', $baseImage)
        if ($base.Code -eq 1) {
            Enter-Stage 'preparing-image' 50 'Podman could not download the pinned builder base image. Check your connection and disk space.'
            if ((Invoke-Native $podman @('--connection', $machine, 'pull', $baseImage)).Code -ne 0) {
                Stop-Setup 50 $stageMessage
            }
        }
        elseif ($base.Code -ne 0) {
            Stop-Setup 50 'Podman could not check the pinned builder base image.'
        }
    }
    Enter-Stage 'verifying-isolation' 42 'Podman could not verify the resource limits required by the isolated builder.'
    if ($imageId) {
        Confirm-ResourceLimits $imageId
    }
    else {
        Confirm-ResourceLimits $baseImage
        Enter-Stage 'preparing-image' 50 'The isolated builder image could not be prepared. Check your network and disk space, then retry.'
        & (Join-Path $PSScriptRoot 'prepare.ps1') -Loader $Loader -AcceptMinecraftEula -BaseImage $baseImage *> $null
        if (-not (Get-BuilderImage $tag)) { Stop-Setup 50 $stageMessage }
    }
    [Console]::WriteLine('BEZALEL_SETUP ready')
}
catch {
    $exitCode = $stageCode
    [Console]::Error.WriteLine($stageMessage)
}
finally {
    if ($locked) { $mutex.ReleaseMutex() }
    if ($mutex) { $mutex.Dispose() }
}
exit $exitCode
