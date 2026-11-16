param(
    [Parameter(Mandatory)][string]$Nsis,
    [Parameter(Mandatory)][string]$NsisScript,
    [string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$payload = Join-Path $root 'apps/installer/src-tauri/payload'
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root 'target/installer-package/output' }
$version = (Get-Content -LiteralPath (Join-Path $root 'apps/desktop/src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$installerVersion = (Get-Content -LiteralPath (Join-Path $root 'apps/installer/src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
if ($version -ne $installerVersion) { throw 'Desktop and installer versions must match' }
& node (Join-Path $PSScriptRoot 'stage-payload.cjs') $Nsis $NsisScript (Join-Path $root 'apps/desktop/src-tauri/binaries/deepagent-install-options.exe') $payload $version
if ($LASTEXITCODE) { throw 'Failed to stage original NSIS payload' }
$previous = $env:DEEPAGENT_INSTALLER_PAYLOAD
try {
    $env:DEEPAGENT_INSTALLER_PAYLOAD = $payload
    Push-Location (Join-Path $root 'apps/installer')
    try { & pnpm build; if ($LASTEXITCODE) { throw 'Installer UI build failed' } } finally { Pop-Location }
    & cargo build --manifest-path (Join-Path $root 'Cargo.toml') --locked --release -p deepagent-installer --bin deepagent-installer --features gui,custom-protocol --target x86_64-pc-windows-msvc --target-dir (Join-Path $root 'target/installer-package')
    if ($LASTEXITCODE) { throw 'Bootstrapper build failed' }
    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $destination = Join-Path $OutputDirectory 'DeepAgent-Studio-Setup-x64.exe'
    Copy-Item -LiteralPath (Join-Path $root 'target/installer-package/x86_64-pc-windows-msvc/release/deepagent-installer.exe') -Destination $destination
    Write-Host "Bootstrapper: $destination"
} finally { $env:DEEPAGENT_INSTALLER_PAYLOAD = $previous }
