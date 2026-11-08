param([string]$ModelDirectory = "$env:APPDATA/ru.gigaam.desktop/models")
$ErrorActionPreference = 'Stop'
$expected = @{
    'v3_e2e_ctc.int8.onnx' = '2e3fcb7a7b66030336fd10c2fcfb033bd1dc7e1bf238fe5cfd83b1d0cfc9d28e'
    'v3_e2e_ctc_vocab.txt' = '142de7570b3de5b3035ce111a89c228e80e6085273731d944093ddf24fa539cd'
    'silero_vad.onnx' = '1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3'
}
foreach ($name in $expected.Keys) {
    $actual = (Get-FileHash (Join-Path $ModelDirectory $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected[$name]) { throw "Pinned checksum mismatch: $name" }
    Write-Output "$name sha256=$actual verified"
}
