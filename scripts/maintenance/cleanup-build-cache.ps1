#Requires -Version 7.0
<#
.SYNOPSIS
    Reports and, with explicit confirmation, removes this repository's
    generated build directories.

.DESCRIPTION
    This repository contains two independent Cargo workspaces: the kernel
    workspace and the Tauri workspace. Their target directories are listed
    explicitly instead of scanning recursively, so reference projects under
    借鉴/ and protected runtime files are never candidates for deletion.

    The default mode is preview-only. Use -Apply and type DELETE when prompted
    to remove generated artifacts. Use -IncludeNodeModules only when the
    frontend dependencies are also intentionally disposable; they are not
    included by default because reinstalling them requires pnpm and may need
    network access.

.EXAMPLE
    pwsh -File .\scripts\maintenance\cleanup-build-cache.ps1

.EXAMPLE
    pwsh -File .\scripts\maintenance\cleanup-build-cache.ps1 -Apply

.EXAMPLE
    pwsh -File .\scripts\maintenance\cleanup-build-cache.ps1 -IncludeNodeModules
#>

[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param(
    [switch]$Apply,
    [switch]$IncludeNodeModules
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path

function Get-DirectoryBytes([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return [int64]0
    }
    $sum = (Get-ChildItem -LiteralPath $Path -Force -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return [int64]0 }
    return [int64]$sum
}

function Format-Bytes([int64]$Bytes) {
    if ($Bytes -ge 1TB) { return ('{0:N2} TB' -f ($Bytes / 1TB)) }
    if ($Bytes -ge 1GB) { return ('{0:N2} GB' -f ($Bytes / 1GB)) }
    if ($Bytes -ge 1MB) { return ('{0:N2} MB' -f ($Bytes / 1MB)) }
    if ($Bytes -ge 1KB) { return ('{0:N2} KB' -f ($Bytes / 1KB)) }
    return ('{0} B' -f $Bytes)
}

function Assert-SafeCandidate([string]$Path) {
    $candidate = [IO.Path]::GetFullPath($Path)
    $rootWithSeparator = $repoRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $candidate.StartsWith($rootWithSeparator, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing a path outside the repository: $candidate"
    }
    $item = Get-Item -LiteralPath $candidate -Force -ErrorAction SilentlyContinue
    if ($null -ne $item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Refusing a reparse-point candidate: $candidate"
    }
}

$relativeCandidates = [System.Collections.Generic.List[string]]::new()
[void]$relativeCandidates.Add('target')
[void]$relativeCandidates.Add('apps\desktop\src-tauri\target')
if ($IncludeNodeModules) {
    [void]$relativeCandidates.Add('apps\desktop\node_modules')
}

$candidates = foreach ($relative in $relativeCandidates) {
    $path = Join-Path $repoRoot $relative
    Assert-SafeCandidate $path
    $bytes = Get-DirectoryBytes $path
    [pscustomobject]@{
        RelativePath = $relative
        FullPath = $path
        Exists = Test-Path -LiteralPath $path -PathType Container
        Bytes = $bytes
        Size = Format-Bytes $bytes
    }
}

$totalBytes = [int64](($candidates | Measure-Object -Property Bytes -Sum).Sum)
Write-Output "Repository: $repoRoot"
Write-Output "Mode: $(if ($Apply) { 'APPLY (deletion requested)' } else { 'PREVIEW ONLY' })"
$candidates | Format-Table RelativePath, Exists, Size -AutoSize
Write-Output ("Potentially reclaimable: {0}" -f (Format-Bytes $totalBytes))

if (-not $Apply) {
    Write-Output 'No files were changed. Re-run with -Apply and type DELETE to remove only the listed generated directories.'
    return
}

if ($totalBytes -eq 0) {
    Write-Output 'Nothing to remove.'
    return
}

$confirmation = Read-Host 'Type DELETE to confirm removal of the listed generated directories'
if ($confirmation -cne 'DELETE') {
    Write-Output 'Confirmation did not match DELETE; nothing was removed.'
    return
}

$removedBytes = [int64]0
foreach ($candidate in $candidates) {
    if (-not $candidate.Exists) { continue }
    if (-not $PSCmdlet.ShouldProcess($candidate.FullPath, 'Remove generated directory')) { continue }
    Remove-Item -LiteralPath $candidate.FullPath -Recurse -Force
    $removedBytes += $candidate.Bytes
    Write-Output ("Removed {0} ({1})" -f $candidate.RelativePath, $candidate.Size)
}

Write-Output ("Removed total: {0}" -f (Format-Bytes $removedBytes))
