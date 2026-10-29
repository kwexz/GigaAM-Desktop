param([string]$EvidenceDirectory = 'state/runs/codec-resources')
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $EvidenceDirectory | Out-Null
$binary = Get-ChildItem 'src-tauri/target/release/deps/gigaam_desktop_lib-*.exe' | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $binary) { throw 'Build release library tests first' }
$stdout = Join-Path (Resolve-Path $EvidenceDirectory) 'stdout.txt'
$stderr = Join-Path (Resolve-Path $EvidenceDirectory) 'stderr.txt'
$started = [DateTime]::UtcNow
$process = Start-Process -FilePath $binary.FullName -ArgumentList @('compare_speech_archive_profiles', '--ignored', '--nocapture') -PassThru -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr
$peak = 0L
while (-not $process.HasExited) {
    try { $process.Refresh(); $peak = [Math]::Max($peak, $process.PeakWorkingSet64) } catch { }
    Start-Sleep -Milliseconds 20
}
$process.Refresh()
@{
    binary = $binary.Name
    binary_sha256 = (Get-FileHash -Algorithm SHA256 $binary.FullName).Hash.ToLowerInvariant()
    base_commit = (git rev-parse HEAD)
    working_tree_dirty = [bool](git status --porcelain)
    exit_code = $process.ExitCode
    started_utc = $started.ToString('o')
    elapsed_ms = ([DateTime]::UtcNow - $started).TotalMilliseconds
    cpu_ms = $process.TotalProcessorTime.TotalMilliseconds
    observed_peak_working_set_bytes = $peak
    scope = 'Whole release benchmark process including fixture loading and all codecs; not per-codec RSS or GUI Stop latency'
} | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $EvidenceDirectory 'resources.json')
if ($process.ExitCode -ne 0) { throw 'Codec benchmark failed' }
