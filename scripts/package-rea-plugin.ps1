#Requires -Version 7.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$NodePath)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression.FileSystem
$repoRoot = Split-Path $PSScriptRoot -Parent
$pluginRoot = Join-Path $repoRoot 'apps/desktop/src-tauri/resources/plugins/rea'
$runtimeRoot = Join-Path $repoRoot 'plugins/rea/runtime'
$node = (Resolve-Path -LiteralPath $NodePath).Path
$nodeBin = Split-Path $node -Parent
$npm = Join-Path $nodeBin 'node_modules/npm/bin/npm-cli.js'
$previousPath = $env:PATH
$stage = Join-Path $repoRoot ('.tmp/rea-package-' + [guid]::NewGuid().ToString('N'))

function Invoke-Node([string[]]$Arguments) {
    & $node @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Node command failed: $($Arguments[0])" }
}

try {
    $platform = & $node -p 'process.platform + "/" + process.arch + "/" + process.version'
    if ($LASTEXITCODE -ne 0 -or $platform -ne 'win32/x64/v24.21.0') {
        throw "This release requires Windows x64 Node 24.21.0; observed: $platform"
    }
    if (-not (Test-Path -LiteralPath $npm -PathType Leaf)) { throw "Missing npm CLI: $npm" }
    $env:PATH = $nodeBin + [IO.Path]::PathSeparator + $previousPath
    $manifest = Get-Content -LiteralPath (Join-Path $pluginRoot '.codex-plugin/plugin.json') -Raw | ConvertFrom-Json
    $lock = Get-Content -LiteralPath (Join-Path $runtimeRoot 'package-lock.json') -Raw | ConvertFrom-Json -AsHashtable
    $reaLock = $lock.packages['node_modules/rea-agents']
    $expectedIntegrity = 'sha512-Vj1zWfSlna5bphGoaoI/TcVBuYvze9u+OyPC3Shz8YkBqaCTqB4COnnRoX7XDwMpwI24ZlFEel2yEj0k8LTWEQ=='
    if ($reaLock.version -ne '6.4.0' -or $reaLock.integrity -ne $expectedIntegrity) {
        throw 'REA lockfile does not match the verified 6.4.0 release'
    }
    Push-Location $runtimeRoot
    try { Invoke-Node @($npm, 'ci', '--omit=dev', '--no-audit', '--no-fund') }
    finally { Pop-Location }
    $reaRoot = Join-Path $runtimeRoot 'node_modules/rea-agents'
    Invoke-Node @((Join-Path $reaRoot 'scripts/verify-windows-native-artifact.mjs'))
    Invoke-Node @('-e', 'const addon = require(process.argv[1]); if (!Object.keys(addon).length) throw new Error("Empty native addon"); console.log("Windows native addon loaded");', (Join-Path $reaRoot 'native/windows/build/rea-windows-x64.node'))
    foreach ($relative in @('scripts/rea.mjs', 'dist/main.js', 'dist/cli.js', 'LICENSE', 'third_party/README.md')) {
        if (-not (Test-Path -LiteralPath (Join-Path $reaRoot $relative) -PathType Leaf)) {
            throw "Missing upstream release resource: $relative"
        }
    }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    foreach ($relative in @('.codex-plugin', '.mcp.json', 'assets', 'skills', 'README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md')) {
        Copy-Item -LiteralPath (Join-Path $pluginRoot $relative) -Destination $stage -Recurse
    }
    Copy-Item -LiteralPath (Join-Path $reaRoot 'LICENSE') -Destination (Join-Path $stage 'LICENSE-REA')
    Copy-Item -LiteralPath (Join-Path $reaRoot 'third_party/README.md') -Destination (Join-Path $stage 'REA-THIRD_PARTY_NOTICES.md')
    $notices = foreach ($entry in $lock.packages.GetEnumerator() | Sort-Object Key) {
        if (-not $entry.Key.StartsWith('node_modules/')) { continue }
        $packageFile = Join-Path $runtimeRoot ($entry.Key + '/package.json')
        if (-not (Test-Path -LiteralPath $packageFile -PathType Leaf)) { continue }
        $package = Get-Content -LiteralPath $packageFile -Raw | ConvertFrom-Json -AsHashtable
        [ordered]@{
            name = $package.name; version = $package.version; path = $entry.Key
            license = $package['license']; integrity = $entry.Value.integrity
        }
    }
    ConvertTo-Json -InputObject @($notices) -Depth 10 | Set-Content -LiteralPath (Join-Path $stage 'DEPENDENCY_NOTICES.json') -Encoding utf8NoBOM
    $runtimeZip = Join-Path $stage 'runtime.zip'
    [IO.Compression.ZipFile]::CreateFromDirectory($runtimeRoot, $runtimeZip, [IO.Compression.CompressionLevel]::Optimal, $false)
    $archive = [IO.Compression.ZipFile]::OpenRead($runtimeZip)
    try {
        $entryCount = $archive.Entries.Count
        $expandedBytes = ($archive.Entries | Measure-Object -Property Length -Sum).Sum
        if ($entryCount -gt 100000 -or $expandedBytes -gt 512MB) { throw 'Runtime exceeds host extraction limits' }
    } finally { $archive.Dispose() }
    $outputRoot = Join-Path $repoRoot 'dist/plugins'
    New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
    $output = Join-Path $outputRoot "rea-plugin-$($manifest.version)-windows-x64.zip"
    if (Test-Path -LiteralPath $output) { Remove-Item -LiteralPath $output }
    [IO.Compression.ZipFile]::CreateFromDirectory($stage, $output, [IO.Compression.CompressionLevel]::Optimal, $false)
    $metadata = [ordered]@{
        pluginVersion = $manifest.version; reaVersion = $reaLock.version
        nodeVersion = '24.21.0'; platform = 'windows-x64'; reaIntegrity = $expectedIntegrity
        runtimeEntries = $entryCount; runtimeExpandedBytes = $expandedBytes
        runtimeSha256 = (Get-FileHash -LiteralPath $runtimeZip -Algorithm SHA256).Hash.ToLowerInvariant()
        lockSha256 = (Get-FileHash -LiteralPath (Join-Path $runtimeRoot 'package-lock.json') -Algorithm SHA256).Hash.ToLowerInvariant()
        packageSha256 = (Get-FileHash -LiteralPath (Join-Path $runtimeRoot 'package.json') -Algorithm SHA256).Hash.ToLowerInvariant()
        zipSha256 = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant()
        zipBytes = (Get-Item -LiteralPath $output).Length
    }
    $metadata | ConvertTo-Json | Set-Content -LiteralPath ($output + '.json') -Encoding utf8NoBOM
    Copy-Item -LiteralPath $runtimeZip -Destination (Join-Path $pluginRoot 'runtime.zip')
    $metadata | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $pluginRoot 'runtime.zip.json') -Encoding utf8NoBOM
    Write-Output $output
    Write-Output ($metadata | ConvertTo-Json)
} finally {
    $env:PATH = $previousPath
    $safeStage = [IO.Path]::GetFullPath($stage)
    $safeParent = [IO.Path]::GetFullPath((Join-Path $repoRoot '.tmp')) + [IO.Path]::DirectorySeparatorChar
    if ($safeStage.StartsWith($safeParent, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $safeStage)) {
        Remove-Item -LiteralPath $safeStage -Recurse -Force
    }
}
