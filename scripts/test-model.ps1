param(
    [string]$AudioPath = 'artifacts/verification/00/generated-speech/speech.wav',
    [string]$ModelDirectory = "$env:APPDATA/ru.gigaam.desktop/models",
    [string]$EvidenceDirectory = 'artifacts/verification/00/model'
)
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot/verify-models.ps1" -ModelDirectory $ModelDirectory
New-Item -ItemType Directory -Force $EvidenceDirectory | Out-Null
$stdout = Join-Path $EvidenceDirectory 'stdout.txt'
$stderr = Join-Path $EvidenceDirectory 'stderr.txt'
$watch = [Diagnostics.Stopwatch]::StartNew()
& cargo run --locked --manifest-path src-tauri/Cargo.toml --features inference-spike --bin inference-spike -- $AudioPath (Join-Path $ModelDirectory 'v3_e2e_ctc.int8.onnx') (Join-Path $ModelDirectory 'v3_e2e_ctc_vocab.txt') (Join-Path $ModelDirectory 'silero_vad.onnx') 1> $stdout 2> $stderr
$code = $LASTEXITCODE
$watch.Stop()
if ($code -ne 0) { throw "Inference failed: exit=$code; see $stderr" }
$text = Get-Content -Raw $stdout
$position = $text.IndexOf('segments=')
if ($position -lt 0) { throw 'Inference output has no segments' }
$segments = $text.Substring($position + 9) | ConvertFrom-Json
$manifest = Get-Content -Raw -Encoding UTF8 (Join-Path (Split-Path $AudioPath) 'manifest.json') | ConvertFrom-Json
$transcript = ($segments | ForEach-Object { $_.text }) -join ' '
foreach ($phrase in $manifest.phrases) {
    if (-not $transcript.Contains($phrase)) { throw "Expected phrase missing: $phrase" }
}
$previousEnd = 0.0
foreach ($segment in $segments) {
    if ($segment.start_seconds -lt $previousEnd -or $segment.end_seconds -le $segment.start_seconds) { throw 'Invalid segment timeline' }
    $previousEnd = $segment.end_seconds
}
[ordered]@{ status = 'verified'; exit_code = $code; elapsed_ms = $watch.ElapsedMilliseconds; phrases_matched = @($manifest.phrases).Count; segments = @($segments).Count; model = 'pinned GigaAM v3 int8 + Silero'; scope = 'file inference, not live E2E' } | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $EvidenceDirectory 'summary.json')
Write-Output "Verified $(@($manifest.phrases).Count) phrases; evidence: $EvidenceDirectory"
