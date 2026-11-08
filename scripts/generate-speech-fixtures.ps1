# Windows PowerShell 5.1: load this UTF-8 script explicitly with ReadAllText.
# System.Speech uses the installed Russian OS voice.
# Generated audio stays local until voice-output redistribution terms are checked.
param([string]$OutputDirectory = 'artifacts/verification/00/generated-speech')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$phrases = @(
    'Сегодня мы проверяем запись русского голоса.',
    'Вторая реплика начинается после короткой паузы.',
    'Все звуковые дорожки должны сохраниться полностью.'
)
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
try {
    $synth.SelectVoiceByHints([System.Speech.Synthesis.VoiceGender]::NotSet, [System.Speech.Synthesis.VoiceAge]::NotSet, 0, [Globalization.CultureInfo]::GetCultureInfo('ru-RU'))
    $format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
    $path = Join-Path (Resolve-Path $OutputDirectory) 'speech.wav'
    $synth.SetOutputToWaveFile($path, $format)
    $prompt = New-Object System.Speech.Synthesis.PromptBuilder
    $prompt.Culture = [Globalization.CultureInfo]::GetCultureInfo('ru-RU')
    foreach ($phrase in $phrases) {
        $prompt.AppendText($phrase)
        $prompt.AppendBreak([TimeSpan]::FromSeconds(1))
    }
    $synth.Speak($prompt)
    $synth.SetOutputToNull()
    [ordered]@{
        voice = $synth.Voice.Name
        provenance = 'Locally synthesized original test text; not private user audio; audio redistribution not authorized by this manifest'
        text_license = 'CC0-1.0'
        phrases = $phrases
        sha256 = (Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()
    } | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory 'manifest.json')
} finally { $synth.Dispose() }
