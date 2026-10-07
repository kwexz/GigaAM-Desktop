<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { convertFileSrc } from '@tauri-apps/api/core';
	import { getCurrentWebview } from '@tauri-apps/api/webview';
	import { open, save } from '@tauri-apps/plugin-dialog';
	import { revealItemInDir } from '@tauri-apps/plugin-opener';
	import { onMount } from 'svelte';

	type Status = 'queued' | 'preparing' | 'transcribing' | 'completed' | 'cancelled' | 'failed' | 'interrupted';
	type Item = { id: string; kind: 'file' | 'live'; source_filename: string; source_path: string; audio_path?: string; created_ms: number; status: Status; error?: string; duration_seconds?: number; completed_chunks: number; total_chunks?: number };
	type Segment = { id: string; index: number; start_seconds: number; end_seconds: number; text: string; speaker_id?: string; source_track: 'file' | 'microphone' | 'system' };

	let items = $state<Item[]>([]);
	let selectedId = $state('');
	let segments = $state<Segment[]>([]);
	let query = $state('');
	let error = $state('');
	let locale = $state<'ru' | 'en'>('ru');
	let segmentRequest = 0;
	let recording = $state(false);
	let modelReady = $state(true);
	let uiNow = $state(Date.now());
	let paused = $state(false);
	let pausedElapsed = $state(0);
	let liveSource = $state<'system_and_microphone' | 'system' | 'microphone'>('system_and_microphone');
	let microphones = $state<string[]>([]);
	let microphone = $state('');
	let micGain = $state(3);
	let micLevel = $state(0);
	let systemLevel = $state(0);
	let liveStarted = $state(0);
	let keepLiveAudio = $state(true);
	let model = $state({ installed: false, downloading: false, downloaded_bytes: 0, total_bytes: 1, error: null as string | null });
	let sidebarMode = $state<'files' | 'record'>('files');
	let busy = $state(false);
	let audio = $state<HTMLAudioElement>();
	let playhead = $state(0);
	let audioPaused = $state(true);
	let audioFiles = $state<string[]>([]);
	let mixedSrc = $state('');
	let mixing = $state(false);
	let confirmDelete = $state(false);
	let editingId = $state<string | null>(null);
	let editText = $state('');

	const copy = {
		ru: { add: 'Добавить файлы', empty: 'Перетащите аудио или видео сюда', timeline: 'Записи', search: 'Поиск', transcript: 'Транскрипт', export: 'Экспорт', reveal: 'Показать файл', remove: 'Удалить', retry: 'Повторить', cancel: 'Отменить', copy: 'Копировать', start: 'Распознать', waiting: 'Выберите запись слева', record: 'Запись', pause: 'Пауза' },
		en: { add: 'Add files', empty: 'Drop audio or video files here', timeline: 'Recordings', search: 'Search', transcript: 'Transcript', export: 'Export', reveal: 'Reveal file', remove: 'Delete', retry: 'Retry', cancel: 'Cancel', copy: 'Copy', start: 'Transcribe', waiting: 'Select a recording', record: 'Record', pause: 'Pause' }
	};
	const statuses = { ru: { queued: 'В очереди', preparing: 'Подготовка', transcribing: 'Распознавание', completed: 'Готово', cancelled: 'Отменено', failed: 'Ошибка', interrupted: 'Прервано' }, en: { queued: 'Queued', preparing: 'Preparing', transcribing: 'Transcribing', completed: 'Completed', cancelled: 'Cancelled', failed: 'Failed', interrupted: 'Interrupted' } };
	let t = $derived(copy[locale]);
	let selected = $derived(items.find((item) => item.id === selectedId));
	let audioSrc = $derived((mixedSrc || audioFiles[0]) ?? selected?.audio_path ?? '');
	function friendlyError(item: Item) {
		if (!item.error) return '';
		if (item.error.includes('Models are not installed')) return locale === 'ru' ? 'Модель не установлена' : 'Model is not installed';
		if (item.error.toLowerCase().includes('decode')) return locale === 'ru' ? 'Формат файла не удалось прочитать' : 'The file format could not be decoded';
		return locale === 'ru' ? 'Обработка завершилась с ошибкой' : 'Processing failed';
	}

	async function refresh() {
		try {
			items = query.trim()
				? await invoke<Item[]>('search_history', { query })
				: await invoke<Item[]>('list_history');
			if (selectedId && !items.some((item) => item.id === selectedId)) selectedId = '';
			if (selectedId) {
				const requestId = selectedId;
				const request = ++segmentRequest;
				const nextSegments = await invoke<Segment[]>('get_segments', { id: requestId });
				if (selectedId === requestId && request === segmentRequest) segments = nextSegments;
			}
			if (selectedId && (selected?.kind === 'live' || !audioFiles.length)) void syncLiveAudio(selectedId);
		} catch {
			// transient failure of a background poll: keep the list already on screen
		}
	}

	async function selectItem(id: string) {
		selectedId = id;
		segments = [];
		audioFiles = [];
		mixedSrc = '';
		mixing = false;
		playhead = 0;
		audioPaused = true;
		void syncLiveAudio(id);
		const request = ++segmentRequest;
		try {
			const nextSegments = await invoke<Segment[]>('get_segments', { id });
			if (selectedId === id && request === segmentRequest) segments = nextSegments;
		} catch {
			// keep the previous (empty) segment list; the next poll will retry
		}
	}

	async function enqueue(paths: string[]) {
		if (!paths.length) return;
		if (!model.installed) { await invoke('download_model'); return; }
		try {
			error = '';
			await invoke('enqueue_files', { paths });
			await refresh();
		} catch { error = locale === 'ru' ? 'Не удалось добавить файлы.' : 'Could not add files.'; }
	}

	async function pickFiles() {
		try {
			const selected = await open({ multiple: true, directory: false, filters: [{ name: 'Media', extensions: ['wav', 'mp3', 'flac', 'ogg', 'm4a', 'mp4', 'mkv', 'avi', 'mov'] }] });
			await enqueue(selected ? (Array.isArray(selected) ? selected : [selected]) : []);
		} catch {
			error = locale === 'ru' ? 'Не удалось открыть диалог выбора файлов.' : 'Could not open the file dialog.';
		}
	}

	async function action(command: string, id: string) {
		try {
			error = '';
			await invoke(command, { id });
			if (command === 'retry_transcription') await invoke('start_transcription', { id });
			if (command === 'cancel_transcription' && selected?.source_path === '') { recording = false; paused = false; }
			selectedId = command === 'delete_history' ? '' : selectedId;
			if (command === 'retry_transcription') segments = [];
			await refresh();
		} catch {
			error = locale === 'ru' ? 'Не удалось выполнить действие.' : 'Could not perform the action.';
		}
	}

	async function reveal(path: string) {
		try {
			error = '';
			await revealItemInDir(path);
		} catch {
			error = locale === 'ru' ? 'Не удалось показать файл. Возможно, он перемещён или удалён.' : 'Could not reveal the file. It may have been moved or deleted.';
		}
	}

	async function requestDelete() {
		if (!selected || busy) return;
		if (['queued', 'preparing', 'transcribing'].includes(selected.status)) {
			error = locale === 'ru' ? 'Активную запись удалить нельзя.' : 'An active recording cannot be deleted.';
			return;
		}
		confirmDelete = true;
	}

	function onKeydown(event: KeyboardEvent) {
		const el = document.activeElement as HTMLElement | null;
		const typing = !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable);
		if (event.key === 'Escape') {
			if (confirmDelete) confirmDelete = false;
			else if (editingId) editingId = null;
			return;
		}
		if (typing || confirmDelete || event.key !== 'Delete') return;
		event.preventDefault();
		requestDelete();
	}

	function exportName() {
		if (!selected) return 'transcript';
		const at = new Date(selected.created_ms);
		const two = (value: number) => String(value).padStart(2, '0');
		const stamp = `${at.getFullYear()}${two(at.getMonth() + 1)}${two(at.getDate())}_${two(at.getHours())}${two(at.getMinutes())}${two(at.getSeconds())}`;
		const dot = selected.source_filename.lastIndexOf('.');
		const stem = dot > 0 ? selected.source_filename.slice(0, dot) : selected.source_filename;
		const extension = dot > 0 ? selected.source_filename.slice(dot) : '.txt';
		return `${stem}_${stamp}${extension}`;
	}

	async function exportResult() {
		if (!selected) return;
		try {
			const path = await save({ defaultPath: exportName(), filters: [{ name: 'Text', extensions: ['txt'] }, { name: 'SubRip', extensions: ['srt'] }, { name: 'WebVTT', extensions: ['vtt'] }, { name: 'JSON', extensions: ['json'] }] });
			if (!path) return;
			const extension = path.split('.').pop()?.toLowerCase();
			const format = extension === 'srt' || extension === 'vtt' || extension === 'json' ? extension : 'txt';
			await invoke('save_export', { id: selected.id, format, path });
		} catch {
			error = locale === 'ru' ? 'Не удалось сохранить файл экспорта.' : 'Could not save the export file.';
		}
	}

	function copyTranscript() {
		copyTranscriptText(segments.map((segment) => `[${time(segment.start_seconds)}] ${segment.text}`).join('\n'));
	}

	async function copyTranscriptText(text: string) {
		try {
			await navigator.clipboard.writeText(text);
		} catch {
			error = locale === 'ru' ? 'Не удалось скопировать текст.' : 'Could not copy the text.';
		}
	}

	function togglePlay(segment: Segment) {
		if (!audio) return;
		const active = !audioPaused && playhead >= segment.start_seconds && playhead < segment.end_seconds;
		if (active) audio.pause();
		else void seek(segment.start_seconds);
	}

	function startEdit(segment: Segment) {
		editingId = segment.id;
		editText = segment.text;
	}

	async function saveEdit(segment: Segment) {
		const text = editText.trim();
		if (!text || !selected) return;
		try {
			await invoke('update_segment_text', { transcriptionId: selected.id, index: segment.index, text });
			segments = segments.map((item) => item.id === segment.id ? { ...item, text } : item);
			editingId = null;
		} catch (err) {
			error = locale === 'ru' ? `Не удалось сохранить правку: ${err}` : `Could not save the edit: ${err}`;
		}
	}

	function editKeys(event: KeyboardEvent, segment: Segment) {
		if (event.key === 'Enter' && !event.shiftKey) {
			event.preventDefault();
			void saveEdit(segment);
		} else if (event.key === 'Escape') {
			editingId = null;
		}
	}

	function duration(seconds?: number) { return seconds == null ? '' : time(seconds); }
	async function syncLiveAudio(id: string) {
		try {
			const files = await invoke<string[]>('session_audio_files', { id });
			if (selectedId !== id || files.join('|') === audioFiles.join('|')) return;
			audioFiles = files;
			if (files.length > 1) void loadMixed(id);
			else mixedSrc = '';
		} catch {
			// keep whatever is already playing
		}
	}
	async function loadMixed(id: string) {
		mixing = true;
		try {
			const mixed = await invoke<string>('mixed_session_audio', { id });
			if (selectedId === id) mixedSrc = mixed;
		} catch (err) {
			if (selectedId === id) {
				mixedSrc = '';
				error = locale === 'ru' ? `Не удалось подготовить общий звук: ${err}` : `Could not prepare mixed audio: ${err}`;
			}
		} finally {
			if (selectedId === id) mixing = false;
		}
	}
	function audioFailed() { error = locale === 'ru' ? 'Не удалось воспроизвести файл. Проверьте, что исходный аудиофайл на месте.' : 'Could not play the file. Make sure the source audio file still exists.'; }
	async function seek(seconds: number) {
		if (!audio) return;
		try {
			audio.currentTime = seconds;
			await audio.play();
		} catch { audioFailed(); }
	}

	async function toggleRecording() {
		if (busy) return;
		busy = true;
		error = '';
		try {
			if (!model.installed) { await invoke('download_model'); return; }
			if (recording) {
				try {
					await invoke('stop_live');
				} catch {
					error = locale === 'ru' ? 'Не удалось остановить запись.' : 'Could not stop recording.';
				}
				recording = false;
				modelReady = true;
				paused = false;
				await refresh();
				return;
			}
			modelReady = false;
			const status = await invoke<{ started_ms: number }>('start_live', { source: liveSource, microphone, keepAudio: keepLiveAudio, micGain });
			liveStarted = status.started_ms;
			recording = true;
			try {
				await refresh();
				const liveItem = items.find((item) => item.status === 'transcribing' && !item.source_path);
				if (liveItem) await selectItem(liveItem.id);
			} catch {
				// запись уже идёт; список подтянется следующим фоновым опросом
			}
		} catch {
			recording = false;
			modelReady = true;
			paused = false;
			error = locale === 'ru' ? 'Не удалось начать запись.' : 'Could not start recording.';
		} finally { busy = false; }
	}

	function openRecordPanel() {
		sidebarMode = sidebarMode === 'record' ? 'files' : 'record';
	}

	async function togglePause() {
		if (busy || !recording) return;
		busy = true;
		try {
			if (paused) {
				await invoke('resume_live');
				liveStarted = Date.now() - pausedElapsed;
				paused = false;
			} else {
				pausedElapsed = Math.max(0, uiNow - liveStarted);
				await invoke('pause_live');
				paused = true;
			}
		} catch {
			error = locale === 'ru' ? 'Не удалось переключить паузу.' : 'Could not toggle pause.';
		} finally { busy = false; }
	}

	function time(seconds: number) {
		return new Date(seconds * 1000).toISOString().slice(11, 19);
	}

	onMount(() => {
		let refreshing = false;
		const poll = () => {
			if (refreshing) return;
			refreshing = true;
			void refresh().finally(() => { refreshing = false; });
		};
		void refresh();
		void invoke<string[]>('list_microphones').then((devices) => { microphones = devices; microphone = devices[0] ?? ''; }).catch(() => {});
		const interval = window.setInterval(poll, 1500);
		const liveInterval = window.setInterval(async () => {
			uiNow = Date.now();
			try {
				model = await invoke<typeof model>('model_status');
				if (!recording) return;
				const status = await invoke<{ microphone_level: number; system_level: number; model_ready: boolean; paused: boolean; error?: string } | null>('live_status');
				if (status) {
					micLevel = status.microphone_level;
					systemLevel = status.system_level;
					if (status.model_ready) modelReady = true;
					paused = status.paused;
					if (status.error) {
						error = locale === 'ru' ? 'Ошибка записи.' : 'Recording failed.';
						recording = false;
						void invoke('stop_live').then(() => refresh()).catch(() => {});
					}
				}
				else recording = false;
			} catch {
				// фоновый опрос: следующую попытку сделает следующий тик таймера
			}
		}, 250);
		const unlisten = getCurrentWebview().onDragDropEvent((event) => {
			if (event.payload.type === 'drop') void enqueue(event.payload.paths);
		});
		return () => { window.clearInterval(interval); window.clearInterval(liveInterval); void unlisten.then((stop) => stop()); };
	});
</script>

<svelte:head><title>GigaAM Desktop</title></svelte:head>
<svelte:window onkeydown={onKeydown} />

<div class="app-shell">
	{#if !model.installed}<div class="model-banner"><div><strong>{locale === 'ru' ? 'Требуется модель распознавания' : 'Recognition model required'}</strong><span>{(model.total_bytes / 1024 / 1024).toFixed(0)} MB</span>{#if model.downloading}<progress max={model.total_bytes} value={model.downloaded_bytes}></progress>{/if}</div>{#if model.downloading}<button onclick={() => invoke('cancel_model_download')}>{t.cancel}</button>{:else}<button onclick={() => invoke('download_model')}>{model.error ? t.retry : locale === 'ru' ? 'Загрузить' : 'Download'}</button>{/if}</div>{/if}
	<header>
		<div class="brand"><span class="mark">G</span><strong>GigaAM</strong><span>Desktop</span></div>
		<div class="header-actions"><button class="ghost" onclick={() => locale = locale === 'ru' ? 'en' : 'ru'}>{locale.toUpperCase()}</button></div>
	</header>

	<aside>
		{#if sidebarMode === 'files'}
			<div class="section-title">{t.timeline}</div>
			<label class="search"><span>⌕</span><input bind:value={query} oninput={() => void refresh()} placeholder={t.search} /></label>
			<div class="list">
				{#each items as item (item.id)}
					<button class="item" class:selected={selectedId === item.id} onclick={() => void selectItem(item.id)}>
						<span class="status" data-status={item.status}></span><span class="item-copy"><strong>{item.source_filename}</strong><small>{item.total_chunks ? `${item.completed_chunks} / ${item.total_chunks} · ` : ''}{new Date(item.created_ms).toLocaleString(locale)}</small></span><em>{statuses[locale][item.status]}</em>
					</button>
				{/each}
				{#if !items.length}<div class="empty-list">{t.empty}</div>{/if}
			</div>
			<div class="sidebar-actions"><button class="add" disabled={!model.installed} onclick={pickFiles}>Открыть</button><button class="record-toggle" onclick={openRecordPanel}>Записать</button></div>
		{:else}
			<div class="record-panel"><div class="section-title">Запись</div><label>Тип источника<select bind:value={liveSource} disabled={recording}><option value="system_and_microphone">System + mic</option><option value="system">System</option><option value="microphone">Microphone</option></select></label>{#if liveSource !== 'system'}<label>Микрофон<select bind:value={microphone} disabled={recording}>{#if !microphones.length}<option value="">Микрофон по умолчанию</option>{/if}{#each microphones as device}<option value={device}>{device}</option>{/each}</select></label>{/if}{#if liveSource !== 'system'}<label>Усиление микрофона<span class="gain-row"><input type="range" min="0.5" max="10" step="0.5" bind:value={micGain} oninput={() => void invoke('set_mic_gain', { micGain }).catch(() => {})} /><b>{micGain.toFixed(1)}×</b></span></label>{/if}<label class="keep"><input type="checkbox" bind:checked={keepLiveAudio} disabled={recording} /> Сохранять FLAC</label><span class="hint">Отдельные lossless дорожки mic/system для дальнейшего анализа.</span>{#if recording}<button class="pause-wide" disabled={busy} onclick={() => void togglePause()}>{paused ? (locale === 'ru' ? 'Продолжить' : 'Resume') : t.pause}</button>{/if}<button class="record-wide" class:active={recording} disabled={busy} onclick={() => void toggleRecording()}>{busy ? (recording ? 'Остановка…' : 'Подготовка…') : recording ? 'Стоп' : '● Запись'}</button><button class="list-wide" disabled={recording || busy} onclick={openRecordPanel}>Список</button></div>
		{/if}
	</aside>

	<main>
		{#if recording}<div class="live-strip"><strong>REC</strong><span>{new Date(Math.max(0, paused ? pausedElapsed : uiNow - liveStarted)).toISOString().slice(11,19)}</span>{#if paused}<span>{locale === 'ru' ? 'Пауза' : 'Paused'}</span>{/if}{#if !modelReady}<span>{locale === 'ru' ? 'Загрузка модели…' : 'Loading model…'}</span>{:else}<span>MIC <i style={`--level:${Math.min(1,micLevel*4)}`}></i></span><span>SYS <i style={`--level:${Math.min(1,systemLevel*4)}`}></i></span>{/if}</div>{/if}
		{#if selected}
			<div class="document-head"><div class="document-meta"><h1>{selected.source_filename}</h1><p>{selected.status === 'completed' ? duration(selected.duration_seconds) : `${statuses[locale][selected.status]}${selected.total_chunks ? ` · ${selected.completed_chunks} / ${selected.total_chunks}` : ''}`}{friendlyError(selected) ? ` · ${friendlyError(selected)}` : ''}</p></div><div class="tools">{#if selected.kind !== 'live' && selected.source_path}<button onclick={() => void reveal(selected.source_path)}>{t.reveal}</button>{:else if selected.kind === 'live' && audioFiles.length}<button onclick={() => void reveal(audioFiles[0] ?? '')}>{t.reveal}</button>{/if}{#if ['queued', 'failed', 'cancelled', 'interrupted', 'completed'].includes(selected.status)}<button class="primary" onclick={() => void action(selected.status === 'queued' ? 'start_transcription' : 'retry_transcription', selected.id)}>{selected.status === 'completed' ? t.retry : t.start}</button>{/if}</div>{#if selected.status !== 'completed' && selected.total_chunks}<progress class="transcribe-progress" max={selected.total_chunks} value={selected.completed_chunks}></progress>{:else if selected.audio_path || audioFiles.length || mixedSrc}<div class="player-row">{#key audioSrc}<audio bind:this={audio} src={convertFileSrc(audioSrc)} ontimeupdate={() => playhead = audio?.currentTime ?? 0} onplay={() => audioPaused = false} onpause={() => audioPaused = true} onerror={audioFailed} controls></audio>{/key}{#if mixing}<span class="mix-hint">{locale === 'ru' ? 'Готовим общий звук…' : 'Preparing mixed audio…'}</span>{/if}</div>{/if}</div>
			<section class="transcript">
				{#each segments as segment (segment.id)}
					<div class="segment" class:playing={playhead >= segment.start_seconds && playhead < segment.end_seconds}><button class="seg-play" title={locale === 'ru' ? 'Слушать / пауза' : 'Play / pause'} aria-label={locale === 'ru' ? 'Слушать / пауза' : 'Play / pause'} onclick={() => togglePlay(segment)}>{!audioPaused && playhead >= segment.start_seconds && playhead < segment.end_seconds ? '⏸' : '▶'}</button><time>{time(segment.start_seconds)}</time><div class="seg-body">{#if segment.source_track !== 'file'}<b>{segment.speaker_id ?? (segment.source_track === 'microphone' ? (locale === 'ru' ? 'Я' : 'Me') : (locale === 'ru' ? 'Система' : 'System'))}</b>{/if}{#if editingId === segment.id}<textarea bind:value={editText} rows={3} onkeydown={(event) => editKeys(event, segment)}></textarea><div class="edit-actions"><button class="primary" onclick={() => void saveEdit(segment)}>{locale === 'ru' ? 'Сохранить' : 'Save'}</button><button class="ghost" onclick={() => editingId = null}>{t.cancel}</button></div>{:else}<button class="seg-text" title={locale === 'ru' ? 'Нажмите, чтобы редактировать' : 'Click to edit'} onclick={() => startEdit(segment)} onkeydown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); startEdit(segment); } }}>{segment.text}</button>{/if}</div></div>
				{/each}
				{#if !segments.length}{#if selected.status === 'completed'}<div class="empty-transcript"><p>{locale === 'ru' ? 'В записи не найдена речь: сегментов нет. Если аудио не пустое, нажмите «Повторить» для перезапуска распознавания.' : 'No speech found in this recording. If the audio is not empty, press Retry to re-run recognition.'}</p></div>{:else}<div class="processing"><span></span><p>{statuses[locale][selected.status]}</p></div>{/if}{/if}
			</section>
			<footer><div class="export"><button onclick={() => void exportResult()}>{t.export}</button><button onclick={() => void copyTranscript()}>{t.copy}</button></div><div class="quick-actions">{#if selected.kind !== 'live' && selected.source_path}<button class="compact-reveal" onclick={() => void reveal(selected.source_path)}>{t.reveal}</button>{:else if selected.kind === 'live' && audioFiles.length}<button class="compact-reveal" onclick={() => void reveal(audioFiles[0] ?? '')}>{t.reveal}</button>{/if}{#if ['queued', 'failed', 'cancelled', 'interrupted', 'completed'].includes(selected.status)}<button class="compact-start" onclick={() => void action(selected.status === 'queued' ? 'start_transcription' : 'retry_transcription', selected.id)}>{selected.status === 'completed' ? t.retry : t.start}</button>{/if}</div><div class="danger">{#if ['failed', 'cancelled', 'interrupted'].includes(selected.status)}<button onclick={() => void action('retry_transcription', selected.id)}>{t.retry}</button>{/if}{#if ['queued', 'preparing', 'transcribing'].includes(selected.status)}<button onclick={() => void action('cancel_transcription', selected.id)}>{t.cancel}</button>{:else}<button onclick={() => requestDelete()}>{t.remove}</button>{/if}</div></footer>
		{:else}
			<div class="welcome"><div class="wave">∿</div><h1>{t.waiting}</h1><p>{t.empty}</p><button class="add-large" disabled={!model.installed} onclick={pickFiles}>＋ {t.add}</button></div>
		{/if}
		{#if error}<div class="toast" role="alert">{error}</div>{/if}
		{#if confirmDelete}{#if selected}<div class="modal-wrap"><button class="modal-backdrop" aria-label={t.cancel} onclick={() => confirmDelete = false}></button><div class="modal" role="dialog" aria-modal="true" tabindex={-1} aria-label={t.remove}><h2>{t.remove}?</h2><p>{selected.source_filename}</p><div class="modal-actions"><button class="danger-solid" onclick={() => { const id = selected.id; confirmDelete = false; void action('delete_history', id); }}>{t.remove}</button><button class="ghost" onclick={() => confirmDelete = false}>{t.cancel}</button></div></div></div>{/if}{/if}
	</main>
</div>

<style>
	:global(*){box-sizing:border-box} :global(html,body){margin:0;height:100%;overflow:hidden} :global(body){font-family:"Segoe UI Variable",Aptos,sans-serif;background:#f2f1ed;color:#20221f} :global(button),:global(input){font:inherit}
	.model-banner{position:fixed;z-index:20;left:50%;top:68px;transform:translateX(-50%);display:flex;align-items:center;gap:18px;padding:12px 16px;border:1px solid #d3c6a5;border-radius:10px;background:#fff8e7;box-shadow:0 8px 30px #0002}.model-banner div{display:grid;gap:4px;min-width:280px}.model-banner span{font-size:11px;color:#777}.model-banner progress{width:100%}.model-banner button{border:0;border-radius:7px;background:#173f35;color:#fff;padding:8px 12px}.app-shell{height:100vh;display:grid;grid-template:58px 1fr/310px 1fr;background:radial-gradient(circle at 88% 8%,#e2eee8 0,transparent 34%),#f7f6f2}
	header{grid-column:1/-1;display:flex;align-items:center;justify-content:space-between;padding:0 18px;border-bottom:1px solid #d8d9d3;background:rgba(248,248,244,.88);backdrop-filter:blur(16px)} .brand{display:flex;align-items:baseline;gap:8px;letter-spacing:-.02em}.brand span:last-child{color:#7b7e76;font-size:13px}.mark{display:grid;place-items:center;width:27px;height:27px;border-radius:8px;background:#123d33;color:#fff;font-family:Georgia,serif}.header-actions{display:flex;gap:8px}.ghost,.tools button,.export button,.danger button{border:1px solid #d5d7d0;background:#fff;border-radius:8px;padding:7px 10px;color:#484b45}.live-strip{display:flex;align-items:center;gap:16px;padding:7px 20px;background:#6f2724;color:#fff;font:11px ui-monospace,monospace}.live-strip span{display:flex;gap:6px;align-items:center}.live-strip i{display:block;width:70px;height:5px;border-radius:3px;background:linear-gradient(90deg,#7fd5a1 calc(var(--level)*100%),#ffffff33 0)}
	aside{min-height:0;border-right:1px solid #d8d9d3;background:#edede8;display:flex;flex-direction:column;padding:14px 10px 10px}.search{display:flex;gap:7px;align-items:center;margin:11px 2px 7px;padding:8px 10px;border:1px solid #d7d8d2;background:#f6f6f2;border-radius:8px;color:#8b8d87}.search input{width:100%;border:0;outline:0;background:transparent}.list{flex:1;overflow:auto}.item{width:100%;border:0;background:transparent;border-radius:10px;padding:10px 9px;display:grid;grid-template-columns:9px 1fr auto;gap:9px;align-items:center;text-align:left;color:#3a3d37}.item:hover,.item.selected{background:#fff}.item-copy{min-width:0}.item strong,.item small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.item strong{font-size:13px;font-weight:600}.item small{margin-top:4px;color:#868982;font-size:11px}.item em{font-size:10px;font-style:normal;color:#85877f}.status{width:7px;height:7px;border-radius:50%;background:#a9aaa4}.status[data-status="completed"]{background:#29715d}.status[data-status="transcribing"],.status[data-status="preparing"]{background:#d18b34;box-shadow:0 0 0 3px #d18b3422}.status[data-status="failed"]{background:#b9584d}.empty-list{padding:28px 15px;text-align:center;color:#898b84;font-size:13px}.add,.add-large{border:0;border-radius:9px;background:#173f35;color:#fff;padding:10px 14px;font-weight:600}.add-large{padding:12px 18px}
	.section-title{font-size:12px;font-weight:700;padding:5px 5px 0}
	.sidebar-actions{display:grid;grid-template-columns:1fr 1fr;gap:7px;margin-top:10px}.record-toggle,.record-wide,.list-wide{border:0;border-radius:9px;background:#dce5df;color:#173f35;padding:10px 12px;font-weight:700}.record-panel{display:flex;flex:1;flex-direction:column;gap:14px;padding:4px}.record-panel label{display:grid;gap:6px;font-size:12px;font-weight:650}.record-panel select{max-width:100%;min-width:0;border:1px solid #d5d7d0;border-radius:8px;background:#fff;padding:9px}.hint{font-size:11px;line-height:1.45;color:#74776f}.record-panel label.keep{display:flex;flex-direction:row;align-items:center;justify-content:space-between}.record-panel label.keep input{width:16px;height:16px;margin:0;accent-color:#173f35;order:2}.gain-row{display:flex;align-items:center;gap:8px}.gain-row input{flex:1;accent-color:#173f35;padding:0}.gain-row b{font-size:12px;min-width:34px;text-align:right}.record-wide{margin-top:auto;width:100%;background:#8f332d;color:#fff}.pause-wide{width:100%;border:0;border-radius:9px;background:#dce5df;color:#173f35;padding:10px 12px;font-weight:700}.record-wide.active{background:#5e2320}.list-wide{width:100%}.transcribe-progress{grid-column:1/-1;display:block;width:100%;height:5px;margin-top:4px;accent-color:#28705a}main{min-width:0;min-height:0;display:flex;flex-direction:column;position:relative}.document-head{padding:10px 19px 12px;display:grid;grid-template-columns:minmax(0,1fr) auto;gap:20px;border-bottom:1px solid #e1e2dc}.player-row{grid-column:1/-1;display:grid;gap:6px}.player-row audio{width:100%;height:34px}.mix-hint{font-size:11px;color:#8b8d87}.document-meta{min-width:0}.document-head h1{margin:0 0 2px;font:500 29px/1.1 Georgia,serif;letter-spacing:-.03em}.document-head p{margin:0;color:#7d8078;font-size:12px}.tools{display:flex;gap:7px;align-items:flex-start}.tools .primary,.compact-start{border:0;background:#173f35;color:#fff;border-radius:8px;padding:8px 12px}.transcript{flex:1;min-width:0;width:100%;overflow:auto;padding:24px 19px 50px}.transcript .segment{width:100%;background:none;border:0;border-bottom:1px solid #e7e7e1;text-align:left;display:grid;grid-template-columns:auto 58px minmax(0,1fr);gap:14px;padding:13px 0;align-items:start}.transcript .segment:hover,.transcript .segment.playing{background:#e5f1eb}.seg-play{width:30px;height:30px;border-radius:50%;border:1px solid #c9cdc2;background:#fff;color:#173f35;font-size:11px;line-height:1;display:grid;place-items:center;padding:0;cursor:pointer}.segment.playing .seg-play{background:#173f35;border-color:#173f35;color:#fff}.seg-body{min-width:0}.seg-body textarea{width:100%;margin-top:5px;font:17px/1.6 Georgia,serif;color:#30332e;background:#fff;border:1px solid #28705a;border-radius:8px;padding:8px;resize:vertical}.edit-actions{display:flex;gap:6px;margin-top:6px}.edit-actions .primary{border:0;background:#173f35;color:#fff;border-radius:8px;padding:8px 12px}.transcript time{font:11px ui-monospace,monospace;color:#999b94;padding-top:8px}.transcript b{font-size:10px;text-transform:uppercase;letter-spacing:.1em;color:#256251}.transcript p{margin:5px 0 0;font:17px/1.6 Georgia,serif;color:#30332e;text-align:left}.seg-body .seg-text{margin:5px 0 0;font:17px/1.6 Georgia,serif;color:#30332e;text-align:left;background:none;border:0;padding:0;cursor:text;width:100%}.processing{display:grid;place-items:center;color:#8a8c85;margin-top:15vh}.empty-transcript{display:grid;place-items:center;margin-top:15vh;color:#8a8c85;text-align:center;padding:0 30px}.empty-transcript p{font-size:14px;line-height:1.6;max-width:420px}.processing span{width:30px;height:30px;border:2px solid #d5d7d0;border-top-color:#286451;border-radius:50%;animation:spin 1s linear infinite}@keyframes spin{to{transform:rotate(360deg)}}footer{display:flex;justify-content:space-between;align-items:center;padding:12px 24px;border-top:1px solid #dcddd7;background:#f5f5f1}.export,.danger,.quick-actions{display:flex;flex-wrap:wrap;gap:6px;align-items:center}.danger button{background:#b3261e;border:1px solid #b3261e;color:#fff}.quick-actions{display:none}.welcome{margin:auto;text-align:center;max-width:440px}.wave{font:72px Georgia;color:#2d6856;line-height:.6}.welcome h1{font:500 28px Georgia,serif;margin:26px 0 8px}.welcome p{color:#7e8179;margin:0 0 24px}.toast{position:absolute;right:20px;bottom:65px;background:#742f29;color:white;padding:10px 14px;border-radius:8px;font-size:12px}.modal-backdrop{position:absolute;inset:0;border:0;padding:0;background:#00000055;cursor:default}.modal-backdrop:hover:not(:disabled){filter:none}.modal-wrap{position:absolute;inset:0;z-index:30;display:grid;place-items:center}.modal{position:relative;background:#fff;border-radius:12px;padding:20px 22px;min-width:300px;max-width:420px;box-shadow:0 12px 40px #0004}.modal h2{margin:0 0 6px;font-size:18px}.modal p{margin:0 0 16px;color:#7d8078;font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.modal-actions{display:flex;gap:8px;justify-content:flex-end}.modal-actions .danger-solid{border:0;border-radius:8px;background:#b3261e;color:#fff;padding:8px 14px;font-weight:600}
	button{transition:background-color .12s ease,border-color .12s ease,filter .12s ease,transform .05s ease}button:hover:not(:disabled){filter:brightness(.94)}button:active:not(:disabled){filter:brightness(.86);transform:translateY(1px)}button:disabled{opacity:.45;cursor:not-allowed}button:focus-visible{outline:2px solid #28705a;outline-offset:2px}
	@media(max-width:720px){.app-shell{grid-template:54px 42% 1fr/1fr}header{grid-row:1}aside{grid-row:2;border-right:0;border-bottom:1px solid #d8d9d3}main{grid-row:3}.document-head{padding:18px 20px}.document-head h1{font-size:22px}.tools{display:none}.quick-actions{display:flex}.transcript{padding:12px 20px}.transcript p,.seg-body .seg-text,.seg-body textarea{font-size:15px}footer{padding:9px 12px;align-items:flex-start;gap:8px;flex-wrap:wrap}.export,.danger,.quick-actions{max-width:100%}}
	@media(prefers-color-scheme:dark){:global(body){background:#1c1e1b;color:#eceee8}.app-shell{background:radial-gradient(circle at 90% 0,#253a32 0,transparent 34%),#20221f}header{background:#20231fe8;border-color:#343833}aside{background:#252824;border-color:#383c36}.item:hover,.item.selected{background:#353a34;color:#f1f2ed}.search{background:#2b2f2a;border-color:#3b403a;color:#dce0d8}.search input{color:#f3f5ef}.search input::placeholder{color:#aeb5aa}.item{color:#e0e2dc}.document-head,footer,.transcript .segment{border-color:#363a35}.document-head p,.welcome p{color:#9ca098}.transcript p,.seg-body .seg-text{color:#e5e7e0}footer{background:#222520}.ghost,.tools button,.export button,.compact-reveal{background:#2c302b;border-color:#40453e;color:#d9dcd4}.record-panel select{background:#2c302b;border-color:#40453e;color:#f4f6f0}.model-banner{background:#2b302b;border-color:#687269;color:#f4f6f0;box-shadow:0 10px 35px #0008}.model-banner strong{color:#fff}.model-banner span{color:#bdc4ba}.model-banner button{background:#d8eadf;color:#153c31;font-weight:700}.model-banner progress{accent-color:#70b99f}.transcript .segment:hover,.transcript .segment.playing{background:#33523f}.transcript time{color:#b9beb4}.seg-body textarea{background:#2c302b;border-color:#70b99f;color:#f4f6f0}.modal{background:#262926;color:#eceee8}.modal p{color:#9ca098}}
</style>
