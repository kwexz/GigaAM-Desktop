param(
    [Parameter(Mandatory = $true)][switch]$AllowDownload,
    [string]$ModelDirectory = 'artifacts/verification/00/models'
)
$ErrorActionPreference = 'Stop'
if (-not $AllowDownload) { throw 'Explicit -AllowDownload required (227 MB)' }
$source = Get-Content -Raw (Join-Path $PSScriptRoot '../src-tauri/src/model_manager.rs')
$pattern = 'name: "([^"]+)",\s+url: "([^"]+)",\s+bytes: ([\d_]+),\s+sha256: "([a-f0-9]+)"'
$artifacts = [regex]::Matches($source, $pattern)
if ($artifacts.Count -ne 3) { throw 'Expected exactly three pinned model artifacts' }
New-Item -ItemType Directory -Force $ModelDirectory | Out-Null
foreach ($artifact in $artifacts) {
    $name = $artifact.Groups[1].Value
    $path = Join-Path $ModelDirectory $name
    $hash = $artifact.Groups[4].Value
    if ((Test-Path $path) -and (Get-FileHash $path).Hash.ToLowerInvariant() -eq $hash) { continue }
    $partial = "$path.partial"
    Invoke-WebRequest -UseBasicParsing -Uri $artifact.Groups[2].Value -OutFile $partial
    if ((Get-Item $partial).Length -ne [long]($artifact.Groups[3].Value.Replace('_', ''))) { throw "Size mismatch: $name" }
    if ((Get-FileHash $partial).Hash.ToLowerInvariant() -ne $hash) { throw "Checksum mismatch: $name" }
    Move-Item -Force $partial $path
}
& "$PSScriptRoot/verify-models.ps1" -ModelDirectory $ModelDirectory
