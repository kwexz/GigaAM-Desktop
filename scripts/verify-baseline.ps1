param([string]$EvidenceDirectory = 'state/runs/00-baseline')

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    New-Item -ItemType Directory -Force $EvidenceDirectory | Out-Null
    $checks = @(
        @{ Name = 'install'; Program = 'npm.cmd'; Arguments = @('ci') },
        @{ Name = 'check'; Program = 'npm.cmd'; Arguments = @('run', 'check') },
        @{ Name = 'unit-web'; Program = 'npm.cmd'; Arguments = @('test') },
        @{ Name = 'build-web'; Program = 'npm.cmd'; Arguments = @('run', 'build') },
        @{ Name = 'unit-rust'; Program = 'cargo'; Arguments = @('test', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--lib') },
        @{ Name = 'integration-rust'; Program = 'cargo'; Arguments = @('test', '--locked', '--manifest-path', 'src-tauri/Cargo.toml') }
    )
    $results = @()
    foreach ($check in $checks) {
        $started = [DateTime]::UtcNow
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $program = $check.Program
        $arguments = $check.Arguments
        $stdout = Join-Path $EvidenceDirectory ($check.Name + '.stdout.txt')
        $stderr = Join-Path $EvidenceDirectory ($check.Name + '.stderr.txt')
        & $program @arguments 1> $stdout 2> $stderr
        $exitCode = $LASTEXITCODE
        $watch.Stop()
        $results += [ordered]@{
            name = $check.Name
            command = "$program $($arguments -join ' ')"
            started_utc = $started.ToString('o')
            elapsed_ms = $watch.ElapsedMilliseconds
            exit_code = $exitCode
            status = $(if ($exitCode -eq 0) { 'verified' } else { 'failed' })
            stdout = $stdout
            stderr = $stderr
        }
        $results | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 (Join-Path $EvidenceDirectory 'results.json')
        Write-Host "$($check.Name): exit=$exitCode elapsed_ms=$($watch.ElapsedMilliseconds)"
    }
    if (@($results | Where-Object { $_.exit_code -ne 0 }).Count -gt 0) { exit 1 }
} finally {
    Pop-Location
}
