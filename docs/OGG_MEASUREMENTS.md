# Ogg/Opus: измерения и незакрытые gates

Дата: 2026-10-28, Windows x64, dirty feature branch. Профиль-кандидат: mono 48 kHz, Opus 24 kbit/s. Исходник каждого сравнения — 1 656 648 samples (34.5135 s). Использованы публичные лицензированные fixtures с атрибуцией в tests/fixtures/besarab.json.

| Fixture | WAV bytes | FLAC bytes | MP3 bytes | Ogg bytes |
|---|---:|---:|---:|---:|
| clean | 3 313 340 | 860 184 | 552 576 | 95 684 |
| noisy | 3 313 340 | 1 393 853 | 552 576 | 93 922 |

Измеренный debug encode Ogg: clean 17 748 ms, noisy 16 325 ms при повторном запуске. Это не release CPU benchmark. Peak RSS и actual Tauri Stop latency этим тестом не измеряются.

Release `--locked` benchmark (тот же input, тест завершён успешно): clean WAV/FLAC/MP3/Ogg соответственно 9/131/281/453 ms, noisy 13/166/259/347 ms. Ogg encoding wall-clock RTF = 0.0131 clean и 0.0101 noisy на этом запуске. Это wall-clock, не process CPU time; RSS/IPC Stop отдельно не измерены.

## Независимый decode

Установленный ffmpeg успешно декодировал оба архива. Он используется только как тестовый oracle, не требуется приложению для кодирования. ffprobe определил Opus/48kHz/mono. Длительность контейнера с учётом pre-skip — 34.520 s; декодированный clean WAV имеет ровно 552 216 samples при 16 kHz, то есть 34.5135 s — исходная длительность сохранена.

## ASR

Все три контрольные фразы найдены pinned GigaAM v3 int8 + Silero в исходном и декодированном clean/noisy. Сравнение нормализованных транскриптов с исходным WAV:

- Clean: 0/304 отличающихся символов, timestamps совпали.
- Noisy: edit distance 3/305 (0.9836% relative CER), timestamps отличаются.

Это relative CER к baseline, не ошибка по размеченному ground truth. Нулевая регрессия на noisy не доказана. Логи: state/runs/ogg-model-clean, ogg-model-noisy, ogg-reference-clean, ogg-reference-noisy; сводка state/runs/ogg-model-comparison.json.

## Решение до завершения проверки

## Дополнительный профиль 48 кбит/с

Release benchmark: clean 186382 bytes / 428 ms; noisy 199135 bytes / 288 ms для тех же 34.5135 s исходной речи. Независимый ffmpeg decode обоих файлов прошёл. Pinned inference нашёл 3/3 контрольные фразы для обоих. Относительно исходного WAV clean CER 0/304, noisy 2/305 = 0.6557%; timestamps отличаются в обоих случаях. Evidence: state/runs/ogg48-model-clean, ogg48-model-noisy, ogg48-model-comparison.json.

Текущий encoder-кандидат повышен до 48 кбит/с. Это уменьшает наблюдаемую noisy-разницу, но не доказывает нулевую регрессию. Он по-прежнему существенно меньше измеренного MP3/FLAC. Default FLAC пока сохранён; native header/compactness assertions необходимо повторить для нового профиля.

## Ресурсный процессный замер

`pwsh -NoProfile -File scripts/measure-codec-resources.ps1` успешно запускает release benchmark binary без включения времени компиляции. Текущий замер: CPU 1109.375 ms, elapsed 1416.0384 ms, observed peak working set 23879680 bytes. Это общий процесс для загрузки двух fixtures и всех четырёх codecs, а не отдельный RSS каждого codec. Evidence: state/runs/codec-resources/resources.json и stdout/stderr рядом. Скрипт не измеряет реальную задержку GUI Stop и не выдаёт её за encoder time.

## Принятое решение: compact default

После проверок выбран Ogg/Opus 48 кбит/с как компактный default для новых предпочтений Desktop. Он меньше всех измеренных пригодных профилей на обеих fixtures; 24 кбит/с отклонён в пользу меньшей наблюдаемой noisy-разницы на48. Разница ASR2/305 не является ground-truth регрессией и не влияет на live PCM inference: архивное кодирование не меняет исходный ASR тракт. Это осознанный lossy компромисс, не обещание побитовой/текстовой идентичности. FLAC остаётся явным lossless выбором, сохранённое предпочтение имеет приоритет, старые архивы не перекодируются.

Release production acknowledgement seam `acknowledgement_does_not_wait_for_slow_archive` проходит: подтверждение Stop не ждёт archive job. Это структурная проверка независимости, не замер реального GUI IPC; GUI latency остаётся not-run. Browser выбирает фактически поддерживаемый Ogg/Opus или WebM fallback, native48 не навязывается неподдерживающему браузеру.

Повторяемость resource evidence: scripts/measure-codec-resources.ps1 теперь записывает SHA-256 фактически запущенного binary, base Git commit и dirty-state. Повторный замер state/runs/codec-resources-current/resources.json: CPU1125ms, elapsed1408.9739ms, peak23920640B, exit0. Binary hash отличает реально измеренный executable от текущего исходника: скрипт сам не пересобирает его и не доказывает совпадение с HEAD.

## Отдельные процессы по codec

После пересборки release binary с фильтром профиля выполнены четыре запуска, каждый обрабатывает обе fixtures только выбранным codec. Все exit0; binary SHA256 `2a0f6203e9cdabb847eaeeb1911cbcee101e3520176594f1773bfe3626a94bad`.

| Codec | CPU ms | Process elapsed ms | Observed peak bytes |
|---|---:|---:|---:|
| WAV | 46.875 | 237.7774 | 22233088 |
| FLAC | 203.125 | 410.3463 | 22806528 |
| MP3 | 390.625 | 525.5975 | 22540288 |
| Ogg48 | 515.625 | 711.9811 | 23330816 |

Включены загрузка/resampling fixtures и test harness; это не isolated encoder allocation. Evidence: state/runs/codec-resources-{wav,flac,mp3,ogg}/{resources.json,stdout.txt,stderr.txt}. GUI Stop latency всё ещё не измерена и не выводится из этих чисел.

Дополнительная компонентная проверка Stop: при намеренно незавершённом mock native acknowledgement переключение RU→EN работает. Это доказывает отзывчивость Svelte-компонента в этом сценарии, а не измеряет реальную задержку Tauri IPC/физического захвата. Тест `keeps the UI responsive while stop acknowledgement is pending` проходит в suite51/51. Реальный GUI gate не закрыт.

Ogg доступен как явно выбираемый lossy формат; FLAC default пока сохранён. Для выбора compact default требуется завершить сопоставление качества, release CPU/RSS и Stop latency, а также принять измеренную noisy-разницу. Нельзя объявлять этот документ доказательством выполнения задачи 5.4 целиком.
