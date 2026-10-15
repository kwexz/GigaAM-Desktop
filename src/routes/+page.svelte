<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { convertFileSrc } from '@tauri-apps/api/core';
	import { getCurrentWebview } from '@tauri-apps/api/webview';
	import { open, save } from '@tauri-apps/plugin-dialog';
	import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
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
	let archiveFormat = $state<'flac' | 'wav' | 'mp3'>('flac');
	let theme = $state<'system' | 'dark' | 'light'>('system');
	let editingNameId = $state('');
	let editingName = $state('');
	let editingNamePlace = $state<'list' | 'header' | ''>('');
	let model = $state({ installed: false, downloading: false, downloaded_bytes: 0, total_bytes: 1, error: null as string | null });
	let sidebarMode = $state<'files' | 'record'>('files');
	let sidebarWidth = $state(310);
	let resizingSidebar = $state(false);
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
	let audioSrc = $derived(mixedSrc || audioFiles[0] || selected?.audio_path || selected?.source_path || '');
	let selectedPath = $derived(selected?.kind === 'live' ? (audioFiles[0] ?? selected?.audio_path ?? '') : (selected?.source_path ?? ''));
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
	function beginRename(item: Item, place: 'list' | 'header') {
		editingNameId = item.id;
		editingName = item.source_filename;
		editingNamePlace = place;
		queueMicrotask(() => document.querySelector<HTMLInputElement>(`.rename-input[data-place="${place}"]`)?.select());
	}
	async function commitRename() {
		const id = editingNameId;
		const name = editingName.trim();
		editingNameId = '';
		editingNamePlace = '';
		if (!id || !name) return;
		try { await invoke('rename_transcription', { id, name }); await refresh(); }
		catch (err) { error = String(err); }
	}
	function setTheme(next: 'system' | 'dark' | 'light') {
		theme = next;
		if (next === 'system') delete document.body.dataset.theme;
		else document.body.dataset.theme = next;
		document.documentElement.style.colorScheme = next === 'system' ? 'light dark' : next;
		localStorage.setItem('theme', next);
	}
	function nextTheme() {
		setTheme(theme === 'system' ? 'light' : theme === 'light' ? 'dark' : 'system');
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
		const dual = segments.some((segment) => segment.source_track === 'microphone') && segments.some((segment) => segment.source_track === 'system');
		copyTranscriptText(segments.map((segment) => `[${time(segment.start_seconds)}] ${dual ? `${segment.source_track === 'microphone' ? (locale === 'ru' ? 'Микрофон' : 'Microphone') : (locale === 'ru' ? 'Система' : 'System')}: ` : ''}${segment.text}`).join('\n'));
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
	function recordingName() {
		const date = new Date();
		const two = (value: number) => String(value).padStart(2, '0');
		return `Live session ${date.getFullYear()}${two(date.getMonth() + 1)}${two(date.getDate())}-${two(date.getHours())}${two(date.getMinutes())}${two(date.getSeconds())}`;
	}
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
			const status = await invoke<{ started_ms: number }>('start_live', { source: liveSource, microphone, archiveFormat, micGain, liveName: recordingName() });
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
		sidebarMode = 'record';
		selectedId = '';
		segments = [];
		audioFiles = [];
		mixedSrc = '';
		segmentRequest++;
	}
	function showList() { sidebarMode = 'files'; }
	function beginSidebarResize(event: PointerEvent) {
		resizingSidebar = true;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}
	function resizeSidebar(event: PointerEvent) {
		if (!resizingSidebar) return;
		sidebarWidth = Math.min(520, Math.max(240, event.clientX));
	}
	function endSidebarResize() { resizingSidebar = false; }
	async function startRecording() {
		if (!recording) await toggleRecording();
	}
	async function stopRecording() {
		if (recording) await toggleRecording();
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
		const savedTheme = localStorage.getItem('theme');
		setTheme(savedTheme === 'light' || savedTheme === 'dark' ? savedTheme : 'system');
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

<svelte:head><title>GigaAM</title></svelte:head>
<svelte:window onkeydown={onKeydown} />

<div class="app-shell" style={`--sidebar-width:${sidebarWidth}px`}>
	{#if !model.installed}<div class="model-banner"><div><strong>{locale === 'ru' ? 'Требуется модель распознавания' : 'Recognition model required'}</strong><span>{(model.total_bytes / 1024 / 1024).toFixed(0)} MB</span>{#if model.downloading}<progress max={model.total_bytes} value={model.downloaded_bytes}></progress>{/if}</div>{#if model.downloading}<button onclick={() => invoke('cancel_model_download')}>{t.cancel}</button>{:else}<button onclick={() => invoke('download_model')}>{model.error ? t.retry : locale === 'ru' ? 'Загрузить' : 'Download'}</button>{/if}</div>{/if}
	<header>
		<div class="brand"><span class="mark">G</span><strong>GigaAM</strong><span>Desktop</span><button class="version-link" title="GitHub" onclick={() => void openUrl('https://github.com/kwexz/GigaAM-Desktop')}>{locale === 'ru' ? 'Версия' : 'Version'} {__APP_VERSION__} b{__BUILD_ID__}</button></div>
		<div class="header-actions"><button class="ghost theme-toggle" title={theme === 'system' ? (locale === 'ru' ? 'Тема: системная' : 'Theme: system') : theme === 'light' ? (locale === 'ru' ? 'Тема: светлая' : 'Theme: light') : (locale === 'ru' ? 'Тема: тёмная' : 'Theme: dark')} onclick={nextTheme}>{theme === 'system' ? '◐' : theme === 'light' ? '☀' : '☾'}</button><button class="ghost" onclick={() => locale = locale === 'ru' ? 'en' : 'ru'}>{locale.toUpperCase()}</button></div>
	</header>

	<aside>
		{#if sidebarMode === 'files'}
			<div class="section-title">{t.timeline}</div>
			<label class="search"><span>⌕</span><input bind:value={query} oninput={() => void refresh()} placeholder={t.search} /></label>
			<div class="list">
				{#each items as item (item.id)}
					<button class="item" class:selected={selectedId === item.id} onclick={() => void selectItem(item.id)} ondblclick={(event) => { event.stopPropagation(); beginRename(item, 'list'); }}>
						<span class="status" data-status={item.status}></span><span class="item-copy">{#if editingNameId === item.id && editingNamePlace === 'list'}<input class="rename-input" data-place="list" bind:value={editingName} onclick={(event) => event.stopPropagation()} onkeydown={(event) => { if (event.key === 'Enter') void commitRename(); if (event.key === 'Escape') { editingNameId = ''; editingNamePlace = ''; } }} onblur={() => void commitRename()} />{:else}<strong>{item.source_filename}</strong>{/if}<small>{item.duration_seconds != null ? `${duration(item.duration_seconds)} · ` : ''}{new Date(item.created_ms).toLocaleString(locale)}</small></span><em>{statuses[locale][item.status]}</em>
					</button>
				{/each}
				{#if !items.length}<div class="empty-list">{t.empty}</div>{/if}
			</div>
			<div class="sidebar-actions"><button class="add" disabled={!model.installed} onclick={pickFiles}>Открыть</button><button class="record-toggle" onclick={openRecordPanel}>Записать</button></div>
		{:else}
			<div class="record-panel"><div class="section-title">{locale === 'ru' ? 'Запись' : 'Recording'}</div><label>{locale === 'ru' ? 'Тип источника' : 'Audio source'}<select bind:value={liveSource} disabled={recording}><option value="system_and_microphone">{locale === 'ru' ? 'Система + микрофон' : 'System + microphone'}</option><option value="system">{locale === 'ru' ? 'Система' : 'System'}</option><option value="microphone">{locale === 'ru' ? 'Микрофон' : 'Microphone'}</option></select></label>{#if liveSource !== 'system'}<label>{locale === 'ru' ? 'Микрофон' : 'Microphone'}<select bind:value={microphone} disabled={recording}>{#if !microphones.length}<option value="">{locale === 'ru' ? 'Микрофон по умолчанию' : 'Default microphone'}</option>{/if}{#each microphones as device}<option value={device}>{device}</option>{/each}</select></label>{/if}{#if liveSource !== 'system'}<label>{locale === 'ru' ? 'Усиление микрофона' : 'Microphone gain'}<span class="gain-row"><input type="range" min="0.5" max="10" step="0.5" bind:value={micGain} oninput={() => void invoke('set_mic_gain', { micGain }).catch(() => {})} /><b>{micGain.toFixed(1)}×</b></span></label>{/if}<label>{locale === 'ru' ? 'Формат записи' : 'Recording format'}<select bind:value={archiveFormat} disabled={recording}><option value="flac">FLAC</option><option value="wav">WAV</option><option value="mp3">MP3</option></select></label><span class="hint">{locale === 'ru' ? 'Отдельные дорожки mic/system сохраняются в выбранном формате.' : 'Separate microphone/system tracks use the selected format.'}</span><div class="transport"><button class="record-action" disabled={recording || busy} onclick={() => void startRecording()}><span class="transport-large">●</span>{locale === 'ru' ? 'Запись' : 'Record'}</button><button class="ghost pause-action" class:pressed={paused} aria-pressed={paused} disabled={!recording || busy} onclick={() => void togglePause()}><span class="pause-bars transport-pause" class:pause-blink={paused} aria-hidden="true"><i></i><i></i></span>{t.pause}</button><button class="ghost" disabled={!recording || busy} onclick={() => void stopRecording()}><span class="transport-large">■</span>{locale === 'ru' ? 'Стоп' : 'Stop'}</button></div><button class="list-wide" disabled={recording || busy} onclick={showList}>{locale === 'ru' ? 'Список' : 'List'}</button></div>
		{/if}
	</aside>
	<div class="sidebar-resizer" role="separator" aria-label={locale === 'ru' ? 'Изменить ширину списка' : 'Resize list'} onpointerdown={beginSidebarResize} onpointermove={resizeSidebar} onpointerup={endSidebarResize} onpointercancel={endSidebarResize}></div>

	<main>
		{#if recording}<div class="live-strip"><strong>REC</strong><span>{new Date(Math.max(0, paused ? pausedElapsed : uiNow - liveStarted)).toISOString().slice(11,19)}</span>{#if paused}<span class="pause-blink">{locale === 'ru' ? 'Пауза' : 'Paused'}</span>{/if}{#if !modelReady}<span>{locale === 'ru' ? 'Загрузка модели…' : 'Loading model…'}</span>{:else}<span>MIC <i style={`--level:${Math.min(1,micLevel*4)}`}></i></span><span>SYS <i style={`--level:${Math.min(1,systemLevel*4)}`}></i></span>{/if}</div>{/if}
		{#if selected}
			<div class="document-head"><div class="document-title">{#if editingNameId === selected.id && editingNamePlace === 'header'}<input class="title-rename rename-input" data-place="header" bind:value={editingName} onkeydown={(event) => { if (event.key === 'Enter') void commitRename(); if (event.key === 'Escape') { editingNameId = ''; editingNamePlace = ''; } }} onblur={() => void commitRename()} />{:else}<h1><button class="title-button" onclick={() => beginRename(selected, 'header')} title={locale === 'ru' ? 'Переименовать' : 'Rename'}>{selected.source_filename}</button></h1>{/if}</div><div class="tools">{#if ['queued', 'failed', 'cancelled', 'interrupted', 'completed'].includes(selected.status)}<button class="primary" onclick={() => void action(selected.status === 'queued' ? 'start_transcription' : 'retry_transcription', selected.id)}>{t.start}</button>{/if}{#if ['preparing', 'transcribing'].includes(selected.status)}<button class="primary" onclick={() => void action('cancel_transcription', selected.id)}>{locale === 'ru' ? 'Остановить' : 'Stop'}</button>{/if}</div><div class="document-meta"><p><span>{selected.status === 'completed' ? duration(selected.duration_seconds) : `${statuses[locale][selected.status]}${selected.total_chunks ? ` · ${selected.completed_chunks} / ${selected.total_chunks}` : ''}`}{friendlyError(selected) ? ` · ${friendlyError(selected)}` : ''}</span>{#if selectedPath}<button class="path-link" title={locale === 'ru' ? 'Показать файл' : 'Show file'} onclick={() => void reveal(selectedPath)}>{selectedPath}</button>{/if}</p></div>{#if selected.status !== 'completed' && selected.total_chunks}<progress class="transcribe-progress" max={selected.total_chunks} value={selected.completed_chunks}></progress>{:else if audioSrc}<div class="player-row">{#key audioSrc}<audio bind:this={audio} src={convertFileSrc(audioSrc)} ontimeupdate={() => playhead = audio?.currentTime ?? 0} onplay={() => audioPaused = false} onpause={() => audioPaused = true} onerror={audioFailed} controls></audio>{/key}{#if mixing}<span class="mix-hint">{locale === 'ru' ? 'Готовим общий звук…' : 'Preparing mixed audio…'}</span>{/if}</div>{/if}</div>
			<section class="transcript">
				{#each segments as segment (segment.id)}
					<div class="segment" class:playing={playhead >= segment.start_seconds && playhead < segment.end_seconds}><button class="seg-play" title={locale === 'ru' ? 'Слушать / пауза' : 'Play / pause'} aria-label={locale === 'ru' ? 'Слушать / пауза' : 'Play / pause'} onclick={() => togglePlay(segment)}>{#if !audioPaused && playhead >= segment.start_seconds && playhead < segment.end_seconds}<span class="pause-bars" aria-hidden="true"><i></i><i></i></span>{:else}<span class="play-triangle" aria-hidden="true">▶</span>{/if}</button><time>{time(segment.start_seconds)}</time><div class="seg-body">{#if segment.source_track !== 'file'}<b>{segment.speaker_id ?? (segment.source_track === 'microphone' ? (locale === 'ru' ? 'Я' : 'Me') : (locale === 'ru' ? 'Система' : 'System'))}</b>{/if}{#if editingId === segment.id}<textarea bind:value={editText} rows={3} onkeydown={(event) => editKeys(event, segment)}></textarea><div class="edit-actions"><button class="primary" onclick={() => void saveEdit(segment)}>{locale === 'ru' ? 'Сохранить' : 'Save'}</button><button class="ghost" onclick={() => editingId = null}>{t.cancel}</button></div>{:else}<button class="seg-text" title={locale === 'ru' ? 'Нажмите, чтобы редактировать' : 'Click to edit'} onclick={() => startEdit(segment)} onkeydown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); startEdit(segment); } }}>{segment.text}</button>{/if}</div></div>
				{/each}
				{#if !segments.length}{#if selected.status === 'completed'}<div class="empty-transcript"><p>{locale === 'ru' ? 'В записи не найдена речь: сегментов нет. Если аудио не пустое, нажмите «Повторить» для перезапуска распознавания.' : 'No speech found in this recording. If the audio is not empty, press Retry to re-run recognition.'}</p></div>{:else}<div class="processing"><span></span><p>{statuses[locale][selected.status]}</p></div>{/if}{/if}
			</section>
			<footer><div class="export"><button onclick={() => void exportResult()}>{t.export}</button><button onclick={() => void copyTranscript()}>{t.copy}</button></div><div class="danger">{#if !['queued', 'preparing', 'transcribing'].includes(selected.status)}<button onclick={() => requestDelete()}>{t.remove}</button>{/if}</div></footer>
		{:else}
			<div class="welcome"><div class="wave">∿</div><h1>{t.waiting}</h1><p>{t.empty}</p><button class="add-large" disabled={!model.installed} onclick={pickFiles}>＋ {t.add}</button></div>
		{/if}
		{#if error}<div class="toast" role="alert">{error}</div>{/if}
		{#if confirmDelete}{#if selected}<div class="modal-wrap"><button class="modal-backdrop" aria-label={t.cancel} onclick={() => confirmDelete = false}></button><div class="modal" role="dialog" aria-modal="true" tabindex={-1} aria-label={t.remove}><h2>{t.remove}?</h2><p>{selected.source_filename}</p><div class="modal-actions"><button class="danger-solid" onclick={() => { const id = selected.id; confirmDelete = false; void action('delete_history', id); }}>{t.remove}</button><button class="ghost" onclick={() => confirmDelete = false}>{t.cancel}</button></div></div></div>{/if}{/if}
	</main>
</div>

<style>
	.document-meta{min-width:0}.document-meta p{display:flex;align-items:center;gap:10px;min-width:0;white-space:nowrap}.document-meta p>span{flex:none}.path-link{display:block;min-width:0;max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;border:0;background:transparent;color:#39705f;padding:0;text-align:left;cursor:pointer}.title-button{max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;border:0;background:transparent;padding:0;color:inherit;font:inherit;letter-spacing:inherit;cursor:text}.rename-input{width:100%;min-width:0;border:1px solid #9da79f;border-radius:5px;background:#fff;padding:3px 6px}.title-rename{font:500 29px/1.1 Georgia,serif}
	:global(*){box-sizing:border-box} :global(html,body){margin:0;height:100%;overflow:hidden} :global(body){font-family:"Segoe UI Variable",Aptos,sans-serif;background:#f2f1ed;color:#20221f} :global(button),:global(input),:global(select){font:inherit}
	.model-banner{position:fixed;z-index:20;left:50%;top:68px;transform:translateX(-50%);display:flex;align-items:center;gap:18px;padding:12px 16px;border:1px solid #d3c6a5;border-radius:10px;background:#fff8e7;box-shadow:0 8px 30px #0002}.model-banner div{display:grid;gap:4px;min-width:280px}.model-banner span{font-size:11px;color:#777}.model-banner progress{width:100%}.app-shell{height:100vh;display:grid;grid-template:58px 1fr/var(--sidebar-width) 1fr;background:radial-gradient(circle at 88% 8%,#e2eee8 0,transparent 34%),#f7f6f2}
	header{grid-column:1/-1;display:flex;align-items:center;justify-content:space-between;padding:0 18px;border-bottom:1px solid #d8d9d3;background:rgba(248,248,244,.88);backdrop-filter:blur(16px)} .brand{display:flex;align-items:baseline;gap:8px;letter-spacing:-.02em;color:inherit}.version-link{border:0;background:transparent;padding:0;color:#557168;font-size:12px;text-decoration:underline;text-underline-offset:2px;cursor:pointer}.brand span:last-child{color:#7b7e76;font-size:13px}.mark{display:grid;place-items:center;width:27px;height:27px;border-radius:8px;background:#123d33;color:#fff;font-family:Georgia,serif}.header-actions{display:flex;gap:8px}.theme-toggle{display:grid;place-items:center;width:36px;height:32px;padding:0!important;font-size:25px;line-height:1}.ghost,.tools button,.export button,.danger button{border:1px solid #d5d7d0;background:#fff;border-radius:8px;padding:7px 10px;color:#484b45}.live-strip{display:flex;align-items:center;gap:16px;padding:7px 20px;background:#6f2724;color:#fff;font:11px ui-monospace,monospace}.live-strip span{display:flex;gap:6px;align-items:center}.live-strip i{display:block;width:70px;height:5px;border-radius:3px;background:linear-gradient(90deg,#7fd5a1 calc(var(--level)*100%),#ffffff33 0)}
	.sidebar-resizer{position:absolute;z-index:8;top:58px;bottom:0;left:calc(var(--sidebar-width) - 3px);width:7px;cursor:col-resize;touch-action:none}.sidebar-resizer:hover,.sidebar-resizer:active{background:#28705a55}aside{min-width:0;min-height:0;border-right:1px solid #d8d9d3;background:#edede8;display:flex;flex-direction:column;padding:14px 10px 10px}.search{display:flex;gap:7px;align-items:center;margin:11px 2px 7px;padding:8px 10px;border:1px solid #d7d8d2;background:#f6f6f2;border-radius:8px;color:#8b8d87}.search input{width:100%;border:0;outline:0;background:transparent}.list{flex:1;overflow:auto}.item{width:100%;border:0;background:transparent;border-radius:10px;padding:10px 9px;display:grid;grid-template-columns:9px 1fr auto;gap:9px;align-items:center;text-align:left;color:#3a3d37}.item:hover,.item.selected{background:#fff}.item-copy{min-width:0}.item strong,.item small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.item strong{font-size:13px;font-weight:600}.item small{margin-top:4px;color:#868982;font-size:11px}.item em{font-size:10px;font-style:normal;color:#85877f}.status{width:7px;height:7px;border-radius:50%;background:#a9aaa4}.status[data-status="completed"]{background:#29715d}.status[data-status="transcribing"],.status[data-status="preparing"]{background:#d18b34;box-shadow:0 0 0 3px #d18b3422}.status[data-status="failed"]{background:#b9584d}.empty-list{padding:28px 15px;text-align:center;color:#898b84;font-size:13px}.add,.add-large{border:0;border-radius:9px;background:#173f35;color:#fff;padding:10px 14px;font-weight:600}.add-large{padding:12px 18px}
	.section-title{font-size:12px;font-weight:700;padding:5px 5px 0}
	.sidebar-actions{display:grid;grid-template-columns:1fr 1fr;gap:7px;margin-top:10px}.record-toggle,.list-wide{border:0;border-radius:9px;background:#dce5df;color:#173f35;padding:10px 12px;font-weight:700}.record-panel{display:flex;flex:1;flex-direction:column;gap:14px;padding:0}.record-panel label{display:grid;gap:6px;font-size:12px;font-weight:650}.record-panel select{max-width:100%;min-width:0;border:1px solid #d5d7d0;border-radius:8px;background:#fff;padding:9px}.hint{font-size:11px;line-height:1.45;color:#74776f}.gain-row{display:flex;align-items:center;gap:8px}.gain-row input{flex:1;accent-color:#173f35;padding:0}.gain-row b{font-size:12px;min-width:34px;text-align:right}.transport{margin-top:auto;display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:6px}.transport button{display:flex;align-items:center;justify-content:center;gap:5px;min-width:0;height:36px;padding:7px 8px;border-radius:8px;font-weight:600}.transport .ghost{border:1px solid #d5d7d0;background:#fff;color:#484b45}.transport .record-action{border:1px solid #8f332d;background:#8f332d;color:#fff}.transport-large{display:flex;align-items:center;justify-content:center;width:20px;height:20px;font:700 28px/20px Arial,sans-serif;transform:translateY(-1px)}.transport-pause{flex:0 0 20px}.pause-action.pressed{background:#d6ddd7;color:#173f35;border-color:#9eaba2;box-shadow:inset 0 2px 4px #0003;transform:translateY(1px)}@keyframes pause-blink{0%,49%{opacity:1}50%,100%{opacity:.2}}.pause-blink{animation:pause-blink 2s steps(1,end) infinite}.list-wide{width:100%}.transcribe-progress{grid-column:1/-1;display:block;width:100%;height:5px;margin-top:4px;accent-color:#28705a}main{min-width:0;min-height:0;display:flex;flex-direction:column;position:relative}.document-head{padding:10px 19px 8px;display:grid;grid-template-columns:minmax(0,1fr) auto;column-gap:20px;row-gap:4px;border-bottom:1px solid #e1e2dc}.document-title{min-width:0}.document-meta{grid-column:1/-1}.player-row{grid-column:1/-1;display:grid;gap:3px;margin-top:0}.player-row audio{width:100%;height:34px}.mix-hint{font-size:11px;color:#8b8d87}.document-meta{min-width:0}.document-head h1{margin:0 0 2px;font:500 29px/1.1 Georgia,serif;letter-spacing:-.03em}.document-head p{margin:0;color:#7d8078;font-size:12px}.tools{display:flex;gap:7px;align-items:flex-start}.transcript{flex:1;min-width:0;width:100%;overflow:auto;padding:24px 19px 50px}.transcript .segment{width:100%;padding-left:8px;background:none;border:0;border-bottom:1px solid #e7e7e1;text-align:left;display:grid;grid-template-columns:auto 58px minmax(0,1fr);gap:18px;padding:13px 8px;align-items:start}.transcript .segment:hover,.transcript .segment.playing{background:#e5f1eb}.seg-play{width:34px;height:34px;margin-right:6px;border-radius:50%;border:1px solid #c9cdc2;background:#fff;color:#173f35;display:grid;place-items:center;padding:0;cursor:pointer}.play-triangle{display:grid;place-items:center;width:20px;height:20px;font:700 17px/1 Arial,sans-serif;padding-left:2px}.pause-bars{display:flex;align-items:center;justify-content:center;gap:4px;width:20px;height:20px}.pause-bars i{display:block;width:4px;height:15px;border-radius:1px;background:currentColor}.segment.playing .seg-play{background:#173f35;border-color:#173f35;color:#fff}.seg-body{min-width:0}.seg-body textarea{width:100%;margin-top:5px;font:17px/1.6 Georgia,serif;color:#30332e;background:#fff;border:1px solid #28705a;border-radius:8px;padding:8px;resize:vertical}.edit-actions{display:flex;gap:6px;margin-top:6px}.transcript time{font:11px ui-monospace,monospace;color:#999b94;padding-top:8px}.transcript b{font-size:10px;text-transform:uppercase;letter-spacing:.1em;color:#256251}.transcript p{margin:5px 0 0;font:17px/1.6 Georgia,serif;color:#30332e;text-align:left}.seg-body .seg-text{margin:5px 0 0;font:17px/1.6 Georgia,serif;color:#30332e;text-align:left;background:none;border:0;padding:0;cursor:text;width:100%}.processing{display:grid;place-items:center;color:#8a8c85;margin-top:15vh}.empty-transcript{display:grid;place-items:center;margin-top:15vh;color:#8a8c85;text-align:center;padding:0 30px}.empty-transcript p{font-size:14px;line-height:1.6;max-width:420px}.processing span{width:30px;height:30px;border:2px solid #d5d7d0;border-top-color:#286451;border-radius:50%;animation:spin 1s linear infinite}@keyframes spin{to{transform:rotate(360deg)}}footer{display:flex;justify-content:space-between;align-items:center;padding:12px 24px;border-top:1px solid #dcddd7;background:#f5f5f1}.export,.danger{display:flex;flex-wrap:wrap;gap:6px;align-items:center}.danger button{background:#b3261e;border:1px solid #b3261e;color:#fff}.welcome{margin:auto;text-align:center;max-width:440px}.wave{font:72px Georgia;color:#2d6856;line-height:.6}.welcome h1{font:500 28px Georgia,serif;margin:26px 0 8px}.welcome p{color:#7e8179;margin:0 0 24px}.toast{position:absolute;right:20px;bottom:65px;background:#742f29;color:white;padding:10px 14px;border-radius:8px;font-size:12px}.modal-backdrop{position:absolute;inset:0;border:0;padding:0;background:#00000055;cursor:default}.modal-backdrop:hover:not(:disabled){filter:none}.modal-wrap{position:absolute;inset:0;z-index:30;display:grid;place-items:center}.modal{position:relative;background:#fff;border-radius:12px;padding:20px 22px;min-width:300px;max-width:420px;box-shadow:0 12px 40px #0004}.modal h2{margin:0 0 6px;font-size:18px}.modal p{margin:0 0 16px;color:#7d8078;font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.modal-actions{display:flex;gap:8px;justify-content:center}.danger-solid{border:1px solid #b3261e!important;background:#b3261e!important;color:#fff!important;border-radius:8px;padding:8px 12px;font-weight:700}.modal-actions
	:where(button:not(.item):not(.seg-text):not(.seg-play):not(.title-button):not(.version-link)){min-height:34px;border:1px solid #cfd3ca;border-radius:8px;background:#fff;color:#343832;padding:7px 11px;font-weight:600}.app-shell button:not(.version-link):not(.title-button):not(.seg-text){font-weight:600}.tools .primary,.primary{background:#173f35;color:#fff;border-color:#173f35}button{transition:background-color .12s ease,border-color .12s ease,filter .12s ease,transform .05s ease}button:hover:not(:disabled){filter:brightness(.94)}button:active:not(:disabled){filter:brightness(.86);transform:translateY(1px)}button:disabled{opacity:.45;cursor:not-allowed}button:focus-visible{outline:2px solid #28705a;outline-offset:2px}
	@media(max-width:720px){.app-shell{grid-template:54px 42% 1fr/1fr}header{grid-row:1}aside{grid-row:2;border-right:0;border-bottom:1px solid #d8d9d3}main{grid-row:3}.document-head{padding:18px 20px}.document-head h1{font-size:22px}.tools{display:none}.transcript{padding:12px 20px}.transcript p,.seg-body .seg-text,.seg-body textarea{font-size:15px}footer{padding:9px 12px;align-items:flex-start;gap:8px;flex-wrap:wrap}.export,.danger{max-width:100%}}
	@media(prefers-color-scheme:dark){:global(body:not([data-theme])){background:#1c1e1b;color:#eceee8}:global(body:not([data-theme])) .app-shell{background:radial-gradient(circle at 90% 0,#253a32 0,transparent 34%),#20221f}:global(body:not([data-theme])) header{background:#20231fe8;border-color:#343833}:global(body:not([data-theme])) aside{background:#252824;border-color:#383c36}:global(body:not([data-theme])) .item:hover,:global(body:not([data-theme])) .item.selected{background:#353a34;color:#f1f2ed}:global(body:not([data-theme])) .search{background:#2b2f2a;border-color:#3b403a;color:#dce0d8}:global(body:not([data-theme])) .search input{color:#f3f5ef}:global(body:not([data-theme])) .item{color:#e0e2dc}:global(body:not([data-theme])) .document-head,:global(body:not([data-theme])) footer,:global(body:not([data-theme])) .transcript .segment{border-color:#363a35}:global(body:not([data-theme])) .transcript p{color:#e5e7e0}:global(body:not([data-theme])) footer{background:#222520}:global(body:not([data-theme])) .ghost,:global(body:not([data-theme])) .tools button,:global(body:not([data-theme])) .export button{background:#2c302b;border-color:#40453e;color:#d9dcd4}:global(body:not([data-theme])) .record-panel select{background:#2c302b;border-color:#40453e;color:#f4f6f0}}
	:global(body[data-theme="dark"]){background:#1c1e1b;color:#eceee8}:global(body[data-theme="dark"]) .app-shell{background:radial-gradient(circle at 90% 0,#253a32 0,transparent 34%),#20221f}:global(body[data-theme="dark"]) header{background:#20231fe8;border-color:#343833}:global(body[data-theme="dark"]) aside{background:#252824;border-color:#383c36}:global(body[data-theme="dark"]) .item:hover,:global(body[data-theme="dark"]) .item.selected{background:#353a34;color:#f1f2ed}:global(body[data-theme="dark"]) .search{background:#2b2f2a;border-color:#3b403a;color:#dce0d8}:global(body[data-theme="dark"]) .search input{color:#f3f5ef}:global(body[data-theme="dark"]) .item{color:#e0e2dc}:global(body[data-theme="dark"]) .document-head,:global(body[data-theme="dark"]) footer,:global(body[data-theme="dark"]) .transcript .segment{border-color:#363a35}:global(body[data-theme="dark"]) .transcript p{color:#e5e7e0}:global(body[data-theme="dark"]) footer{background:#222520}:global(body[data-theme="dark"]) .ghost,:global(body[data-theme="dark"]) .tools button,:global(body[data-theme="dark"]) .export button{background:#2c302b;border-color:#40453e;color:#d9dcd4}:global(body[data-theme="dark"]) .record-panel select{background:#2c302b;border-color:#40453e;color:#f4f6f0}
	:global(body[data-theme="dark"]) .seg-text{color:#eef1eb}:global(body[data-theme="dark"]) .transcript .segment:hover,:global(body[data-theme="dark"]) .transcript .segment.playing{background:#2b4035}:global(body[data-theme="dark"]) .transcript .segment:hover .seg-text,:global(body[data-theme="dark"]) .transcript .segment.playing .seg-text{color:#f6f8f3}
	:global(body[data-theme="dark"]) .rename-input,:global(body[data-theme="dark"]) .title-rename{background:#2c302b;color:#f4f6f0;border-color:#697269;caret-color:#fff}:global(body[data-theme="dark"]) .rename-input::selection{background:#477a66;color:#fff}
	:global(body[data-theme="dark"]) .modal{background:#262926;color:#eceee8;border:1px solid #40453e}:global(body[data-theme="dark"]) .modal p{color:#aeb5aa}
	@media(prefers-color-scheme:dark){:global(body:not([data-theme])) .seg-text{color:#eef1eb}:global(body:not([data-theme])) .transcript .segment:hover,:global(body:not([data-theme])) .transcript .segment.playing{background:#2b4035}:global(body:not([data-theme])) .transcript .segment:hover .seg-text,:global(body:not([data-theme])) .transcript .segment.playing .seg-text{color:#f6f8f3}}
	@media(prefers-color-scheme:dark){:global(body:not([data-theme])) .rename-input,:global(body:not([data-theme])) .title-rename{background:#2c302b;color:#f4f6f0;border-color:#697269;caret-color:#fff}:global(body:not([data-theme])) .rename-input::selection{background:#477a66;color:#fff}}
	@media(prefers-color-scheme:dark){:global(body:not([data-theme])) .modal{background:#262926;color:#eceee8;border:1px solid #40453e}:global(body:not([data-theme])) .modal p{color:#aeb5aa}}
</style>
