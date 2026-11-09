import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Page from './+page.svelte';
import * as browserStorage from '../lib/browser-storage';

const { invoke, open, save, revealItemInDir, openUrl, onDragDropEvent } = vi.hoisted(() => ({
	invoke: vi.fn(),
	open: vi.fn(),
	save: vi.fn(),
	revealItemInDir: vi.fn(),
	openUrl: vi.fn(),
	onDragDropEvent: vi.fn().mockResolvedValue(() => {})
}));

vi.mock('@tauri-apps/api/core', () => ({
	invoke,
	convertFileSrc: (path: string) => `asset://${path}`
}));
vi.mock('@tauri-apps/api/webview', () => ({
	getCurrentWebview: () => ({ onDragDropEvent })
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open, save }));
vi.mock('@tauri-apps/plugin-opener', () => ({ revealItemInDir, openUrl }));

type Item = {
	id: string;
	kind: 'file' | 'live';
	source_filename: string;
	source_path: string;
	created_ms: number;
	status: 'completed';
	completed_chunks: number;
};

const historyItem: Item = {
	id: 'tx-1',
	kind: 'file',
	source_filename: 'meeting.wav',
	source_path: 'C:\\audio\\meeting.wav',
	created_ms: 1_700_000_000_000,
	status: 'completed',
	completed_chunks: 1
};

function mockBackend(options: { installed?: boolean; items?: Item[] } = {}) {
	const installed = options.installed ?? true;
	const items = options.items ?? [];
	invoke.mockImplementation(async (command: string) => {
		switch (command) {
			case 'list_history': return items;
			case 'load_organization': return [];
			case 'list_queue': return [];
			case 'model_status': return { installed, downloading: false, downloaded_bytes: 0, total_bytes: 227_222_878, error: null };
			case 'list_microphones': return ['Test microphone'];
			case 'live_status': return null;
			case 'get_segments': return [{ id: 'tx-1:0', index: 0, start_seconds: 1.25, end_seconds: 2.5, text: 'Проверочный текст', source_track: 'file' }];
			case 'session_audio_files': return [];
			default: return undefined;
		}
	});
}

describe('GigaAM page', () => {
	it('highlights a grouped transcript while its audio is playing', async () => {
		mockBackend({ items: [historyItem] });
		render(Page);
		await fireEvent.click(await screen.findByText('meeting.wav'));
		const text = await screen.findByRole('button', { name: 'Проверочный текст' });
		const audio = document.querySelector('audio')!;
		Object.defineProperty(audio, 'currentTime', { value: 1.5, configurable: true });
		await fireEvent.timeUpdate(audio);
		expect(text.closest('.segment')).toHaveClass('playing');
	});
	it('opens model settings when transcription with diarization has no model', async () => {
		mockBackend({ items: [historyItem] });
		render(Page);
		await fireEvent.click(await screen.findByText('meeting.wav'));
		await fireEvent.click(screen.getByText('Распознать с диаризацией', { selector: 'button' }));
		expect(await screen.findByRole('button', { name: /Загрузить модель говорящих/ })).toBeTruthy();
		expect(invoke).not.toHaveBeenCalledWith('retry_transcription', expect.anything());
	});
	it('keeps microphone and system labels alongside speaker labels', async () => {
		mockBackend({ items: [historyItem] });
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'get_segments' ? [
			{ id: 'm', index: 0, start_seconds: 0, end_seconds: 1, text: 'Привет.', source_track: 'microphone', speaker_id: 'Speaker 1' },
			{ id: 's', index: 1, start_seconds: 1, end_seconds: 2, text: 'Здравствуйте.', source_track: 'system', speaker_id: 'Speaker 2' }
		] : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByText('meeting.wav'));
		expect(await screen.findByText('Speaker 1')).toBeTruthy();
		expect(screen.getByText('Speaker 2')).toBeTruthy();
	});
	it('shows speaker labels on imported files', async () => {
		mockBackend({ items: [historyItem] });
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'get_segments' ? [
			{ id: 'a', index: 0, start_seconds: 0, end_seconds: 1, text: 'Привет.', source_track: 'file', speaker_id: 'Speaker 1' }
		] : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByText('meeting.wav'));
		expect(await screen.findByText('Speaker 1')).toBeTruthy();
	});
	it('groups phrase fragments without changing their editable segments', async () => {
		mockBackend({ items: [historyItem] });
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'get_segments' ? [
			{ id: 'a', index: 0, start_seconds: 1, end_seconds: 2, text: 'Начало фразы', source_track: 'file' },
			{ id: 'b', index: 1, start_seconds: 2, end_seconds: 3, text: 'её окончание.', source_track: 'file' },
			{ id: 'c', index: 2, start_seconds: 3, end_seconds: 4, text: 'Другая фраза.', source_track: 'file' }
		] : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByText('meeting.wav'));
		await fireEvent.click(await screen.findByRole('button', { name: 'Начало фразы её окончание. Другая фраза.' }));
		expect((document.querySelector('textarea') as HTMLTextAreaElement).value).toBe('Начало фразы её окончание. Другая фраза.');
	});
	it('does not show one recording’s summary duration on another recording', async () => {
		const second = { ...historyItem, id: 'tx-2', source_filename: 'second.wav' };
		mockBackend({ items: [historyItem, second] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'summarize_ollama') return 'Summary';
			if (command === 'load_summary') return '';
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByText('meeting.wav'));
		await fireEvent.click(await screen.findByRole('switch'));
		await fireEvent.click(await screen.findByRole('button', { name: 'Суммаризовать' }));
		expect(await screen.findByText(/Время суммаризации/)).toBeTruthy();
		await fireEvent.click(await screen.findByText('second.wav'));
		await waitFor(() => expect(screen.queryByText(/Время суммаризации/)).toBeNull());
	});
	it('restores a saved Ollama model without replacing it when the list loads', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'ollama', endpoint: 'https://example.invalid/v1', model: '', ollama: 'custom:latest', gguf: '' }));
		mockBackend();
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'list_summary_models' ? ['other:latest', 'custom:latest'] : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		const picker = screen.getByRole('combobox', { name: 'Модель Ollama' }) as HTMLSelectElement;
		expect(picker.value).toBe('custom:latest');
		await fireEvent.click(screen.getByRole('button', { name: 'Обновить' }));
		await waitFor(() => expect(screen.getByText('other:latest')).toBeTruthy());
		expect(picker.value).toBe('custom:latest');
		await fireEvent.input(screen.getByPlaceholderText('Поиск модели'), { target: { value: 'other' } });
		expect(picker.value).toBe('custom:latest');
		localStorage.removeItem('gigaam-summary-settings');
	});
	it('saves the selected Ollama model and restores it after remount', async () => {
		localStorage.removeItem('gigaam-summary-settings');
		mockBackend(); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'list_summary_models' ? ['model-one', 'model-two'] : backend(command, ...args));
		const first = render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Обновить' }));
		await waitFor(() => expect(screen.getByText('model-two')).toBeTruthy());
		await fireEvent.change(screen.getByRole('combobox', { name: 'Модель Ollama' }), { target: { value: 'model-two' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Сохранить' }));
		expect(JSON.parse(localStorage.getItem('gigaam-summary-settings')!).ollama).toBe('model-two');
		first.unmount(); render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		expect((screen.getByRole('combobox', { name: 'Модель Ollama' }) as HTMLSelectElement).value).toBe('model-two');
		localStorage.removeItem('gigaam-summary-settings');
	});
	it('preserves the saved Ollama model if refresh returns no models', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'ollama', endpoint: 'https://example.invalid/v1', model: '', ollama: 'saved:latest', gguf: '' }));
		mockBackend(); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'list_summary_models' ? [] : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Обновить' }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('list_summary_models'));
		expect((screen.getByRole('combobox', { name: 'Модель Ollama' }) as HTMLSelectElement).value).toBe('saved:latest');
	});
	it('restores API settings and cancels edits without storing a secret', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'api', endpoint: 'https://example.invalid/v1', model: 'saved-model', ollama: 'local-model', gguf: 'local.gguf' }));
		mockBackend(); render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		const endpoint = screen.getByLabelText('API URL') as HTMLInputElement;
		expect(endpoint.value).toBe('https://example.invalid/v1');
		await fireEvent.input(endpoint, { target: { value: 'https://changed.invalid/v1' } });
		await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'example-secret-value' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Отмена' }));
		expect(localStorage.getItem('gigaam-summary-settings')).not.toContain('example-secret-value');
		await fireEvent.click(screen.getByRole('button', { name: 'Настройки' }));
		expect((screen.getByLabelText('API URL') as HTMLInputElement).value).toBe('https://example.invalid/v1');
		localStorage.removeItem('gigaam-summary-settings');
	});
	it('saves a selected GGUF path and restores it after remount', async () => {
		mockBackend(); open.mockResolvedValueOnce('C:\\models\\example.gguf');
		const first = render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.change(screen.getByLabelText('Источник'), { target: { value: 'gguf' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Выбрать' }));
		await waitFor(() => expect((screen.getByLabelText('Файл модели') as HTMLInputElement).value).toBe('C:\\models\\example.gguf'));
		await fireEvent.click(screen.getByRole('button', { name: 'Сохранить' }));
		first.unmount(); render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		expect((screen.getByLabelText('Файл модели') as HTMLInputElement).value).toBe('C:\\models\\example.gguf');
	});
	it('cancels GGUF selection without changing persisted model', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'gguf', endpoint: 'https://example.invalid/v1', model: '', ollama: 'local-model', gguf: 'C:\\models\\original.gguf' }));
		mockBackend(); open.mockResolvedValueOnce('C:\\models\\replacement.gguf'); render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Выбрать' }));
		await waitFor(() => expect((screen.getByLabelText('Файл модели') as HTMLInputElement).value).toBe('C:\\models\\replacement.gguf'));
		await fireEvent.click(screen.getByRole('button', { name: 'Отмена' }));
		expect(JSON.parse(localStorage.getItem('gigaam-summary-settings')!).gguf).toBe('C:\\models\\original.gguf');
		await fireEvent.click(screen.getByRole('button', { name: 'Настройки' }));
		expect((screen.getByLabelText('Файл модели') as HTMLInputElement).value).toBe('C:\\models\\original.gguf');
	});
	it('keeps the settings dialog open when credential storage fails', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'api', endpoint: 'https://example.invalid/v1', model: 'saved-model', ollama: 'local', gguf: '' }));
		mockBackend(); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => { if (command === 'set_summary_api_key') throw new Error('store unavailable'); return backend(command, ...args); });
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'test-key' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Сохранить' }));
		await waitFor(() => expect(screen.getByText('Не удалось сохранить настройки')).toBeTruthy());
		expect(screen.getByLabelText('API key')).toBeTruthy();
		expect(localStorage.getItem('gigaam-summary-settings')).not.toContain('test-key');
	});
	it('saves an API model without persisting its key in browser storage', async () => {
		mockBackend();
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'list_summary_api_models' ? ['demo-model'] : backend(command, ...args));
		const first = render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.change(screen.getByLabelText('Источник'), { target: { value: 'api' } });
		await fireEvent.input(screen.getByLabelText('API URL'), { target: { value: 'https://example.invalid/v1' } });
		await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'test-secret' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Проверить подключение и получить модели' }));
		await waitFor(() => expect(screen.getByText('demo-model')).toBeTruthy());
		await fireEvent.change(screen.getAllByRole('combobox').at(-1)!, { target: { value: 'demo-model' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Сохранить' }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('set_summary_api_key', { key: 'test-secret', endpoint: 'https://example.invalid/v1' }));
		expect(localStorage.getItem('gigaam-summary-settings')).not.toContain('test-secret');
		first.unmount(); render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		expect((screen.getByLabelText('API URL') as HTMLInputElement).value).toBe('https://example.invalid/v1');
		expect((screen.getByText('demo-model') as HTMLOptionElement).selected).toBe(true);
	});
	it('filters API models without changing the selected model', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'api', endpoint: 'https://example.invalid/v1', model: 'model-a', ollama: 'local', gguf: '' }));
		mockBackend();
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'list_summary_api_models' ? ['model-a', 'model-b'] : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Проверить подключение и получить модели' }));
		await waitFor(() => expect(screen.getByText('model-b')).toBeTruthy());
		await fireEvent.input(screen.getByPlaceholderText('Поиск модели'), { target: { value: 'model-b' } });
		expect((screen.getByText('model-a') as HTMLOptionElement).selected).toBe(true);
	});
	it('does not replace a saved API model missing from a refreshed list', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'api', endpoint: 'https://example.invalid/v1', model: 'previous-model', ollama: 'local', gguf: '' }));
		mockBackend(); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'list_summary_api_models' ? ['another-model'] : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Проверить подключение и получить модели' }));
		await waitFor(() => expect(screen.getByText('another-model')).toBeTruthy());
		expect((screen.getByText('previous-model') as HTMLOptionElement).selected).toBe(true);
	});
	it('ignores a late API model response after settings are cancelled', async () => {
		localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'api', endpoint: 'https://example.invalid/v1', model: 'saved-model', ollama: 'local', gguf: '' }));
		mockBackend(); const backend = invoke.getMockImplementation()!;
		let resolveModels!: (models: string[]) => void;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => command === 'list_summary_api_models' ? new Promise<string[]>((resolve) => { resolveModels = resolve; }) : backend(command, ...args));
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Настройки' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Суммаризация' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Проверить подключение и получить модели' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Отмена' }));
		resolveModels(['late-model']);
		await fireEvent.click(screen.getByRole('button', { name: 'Настройки' }));
		expect(screen.queryByText('late-model')).toBeNull();
		expect((screen.getByText('saved-model') as HTMLOptionElement).selected).toBe(true);
	});
	it('shows both concurrent Drive branches without overwriting automatically', async () => {
		vi.spyOn(window, 'confirm').mockReturnValue(true);
		mockBackend({ items: [historyItem] });
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_tree') return { snapshots: await invoke('google_drive_snapshots'), branches: await invoke('google_drive_branches') };
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_restore') return 'tx-1';
			if (command === 'google_drive_snapshots') return [{ id: 'left-file', name: 'Left snapshot' }, { id: 'right-file', name: 'Right snapshot' }];
			if (command === 'google_drive_branches') return [{ file_id: 'left-file', original_id: 'original', revision: 'left', concurrent: true }, { file_id: 'right-file', original_id: 'original', revision: 'right', concurrent: true }];
			return backend(command, ...args);
		});
		render(Page);
		await fireEvent.click(await screen.findByText('meeting.wav'));
		await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect(await screen.findAllByText('Конкурирующие версии — обе сохранены')).toHaveLength(2);
		expect(screen.getAllByRole('button', { name: 'Восстановить эту версию отдельно' })).toHaveLength(2);
		expect(invoke).not.toHaveBeenCalledWith('google_drive_restore', expect.anything());
		await fireEvent.click(screen.getAllByRole('button', { name: 'Восстановить эту версию отдельно' })[0]);
		await fireEvent.click(await screen.findByRole('button', { name: 'Опубликовать как решение конфликта' }));
		expect(invoke).not.toHaveBeenCalledWith('google_drive_resolve', expect.anything());
		await fireEvent.click(screen.getByRole('button', { name: /^Опубликовать$/ }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('google_drive_resolve', { id: 'tx-1', parents: ['left', 'right'] }));
	});
	it('does not keep stale Drive snapshots after a failed refresh', async () => {
		mockBackend({ items: [historyItem] });
		const backend = invoke.getMockImplementation()!;
		let fail = false;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_tree') return { snapshots: await invoke('google_drive_snapshots'), branches: await invoke('google_drive_branches') };
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_snapshots') { if (fail) throw new Error('discovery failed'); return [{ id: 'snapshot', name: 'old-backup.json' }]; }
			if (command === 'google_drive_branches') return [];
			return backend(command, ...args);
		});
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect(await screen.findByText(/old-backup\.json/)).toBeTruthy();
		fail = true;
		await fireEvent.click(screen.getByRole('button', { name: 'Обновить' }));
		await waitFor(() => expect(screen.queryByText(/old-backup\.json/)).toBeNull());
		expect(screen.getAllByText(/discovery failed/).length).toBeGreaterThan(0);
		expect(screen.queryByText('Резервных копий не найдено')).toBeNull();
	});
	it('queues selected local recordings through the durable Drive command', async () => {
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_tree') return { snapshots: [], branches: [] };
			if (command === 'google_drive_restored_ids' || command === 'google_drive_local_lineage') return {};
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByText('meeting.wav')); await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		await fireEvent.click(await screen.findByRole('checkbox', { name: /meeting\.wav/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Отправить выбранное' }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('google_drive_upload', { id: 'tx-1' }));
	});
	it('shows empty remote folders from a validated snapshot organization', async () => {
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_tree') return { snapshots: await invoke('google_drive_snapshots'), branches: await invoke('google_drive_branches') };
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_snapshots') return [{ id: 'snapshot', name: 'backup.json' }];
			if (command === 'google_drive_branches') return [{ file_id: 'snapshot', original_id: 'original', revision: 'r1', concurrent: false, name: 'audio', organization_recording_id: 'original', organization: [{ id: 'folder', kind: 'folder', parent_id: null, name: 'Empty folder', position: 0 }] }];
			if (command === 'google_drive_restored_ids' || command === 'google_drive_local_lineage') return {};
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByText('meeting.wav')); await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect(await screen.findByText(/Empty folder/)).toBeTruthy();
	});
	it('keeps an empty nested folder visible alongside conflicting snapshots', async () => {
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		const organization = [
			{ id: 'parent', kind: 'folder', parent_id: null, name: 'Parent', position: 0 },
			{ id: 'empty', kind: 'folder', parent_id: 'parent', name: 'Empty nested', position: 0 }
		];
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_tree') return {
				snapshots: [{ id: 'left', name: 'left.json' }, { id: 'right', name: 'right.json' }],
				branches: ['left', 'right'].map((id) => ({ file_id: id, original_id: 'original', revision: id, concurrent: true, is_head: true, organization }))
			};
			if (command === 'google_drive_restored_ids' || command === 'google_drive_local_lineage') return {};
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect(await screen.findAllByText('Конкурирующие версии — обе сохранены')).toHaveLength(2);
		expect(screen.getAllByText(/Parent \/ Empty nested/)).toHaveLength(2);
		expect(invoke).not.toHaveBeenCalledWith('google_drive_restore', expect.anything());
	});
	it('opens Drive as a screen and returns to recordings', async () => {
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_tree') return { snapshots: [], branches: [] };
			if (command === 'google_drive_restored_ids' || command === 'google_drive_local_lineage') return {};
			return backend(command, ...args);
		});
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect(await screen.findByRole('region', { name: 'Google Drive backups' })).toBeTruthy();
		expect(screen.queryByRole('dialog', { name: 'Google Drive backups' })).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: /К записям/ }));
		expect(screen.queryByRole('region', { name: 'Google Drive backups' })).toBeNull();
		expect(await screen.findByText('meeting.wav')).toBeTruthy();
	});
	it('combines a local recording and its exact Drive revision in one row', async () => {
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_tree') return { snapshots: [{ id: 'file-1', name: 'snapshot.json' }], branches: [{ file_id: 'file-1', original_id: 'original', revision: 'rev', concurrent: false }] };
			if (command === 'google_drive_local_lineage') return { 'tx-1': { original_id: 'original', revision: 'rev' } };
			if (command === 'google_drive_restored_ids') return {};
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByText('meeting.wav')); await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect(await screen.findByLabelText('Версия найдена в Drive, актуальность неизвестна')).toBeTruthy();
		expect(screen.getByRole('checkbox', { name: /Выбрать снимок Drive: snapshot.json/ })).toBeTruthy();
		expect(screen.queryByText(/snapshot.json · снимок Drive/)).toBeNull();
	});
	it('continues a bulk restore after a failure and skips successful copies on retry', async () => {
		vi.spyOn(window, 'confirm').mockReturnValue(true);
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!; let fail = true;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_tree') return { snapshots: [{ id: 'one', name: 'one.json' }, { id: 'two', name: 'two.json' }], branches: [] };
			if (command === 'google_drive_restored_ids' || command === 'google_drive_local_lineage') return {};
			if (command === 'google_drive_restore') { const id = (args[0] as { fileId: string }).fileId; if (id === 'one' && fail) throw new Error('network'); return `local-${id}`; }
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		await fireEvent.click(await screen.findByRole('checkbox', { name: /one\.json/ }));
		await fireEvent.click(screen.getByRole('checkbox', { name: /two\.json/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Скачать выбранное' }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('google_drive_restore', { fileId: 'two' }));
		expect(await screen.findByText(/Восстановлено: 1, ошибок: 1/)).toBeTruthy();
		fail = false; await fireEvent.click(screen.getByRole('button', { name: 'Скачать выбранное' }));
		await waitFor(() => expect(invoke.mock.calls.filter(([command, args]) => command === 'google_drive_restore' && args?.fileId === 'one')).toHaveLength(2));
		expect(invoke.mock.calls.filter(([command, args]) => command === 'google_drive_restore' && args?.fileId === 'two')).toHaveLength(1);
	});
	it('restores a saved selection without letting an empty SQLite fallback overwrite it', async () => {
		localStorage.setItem('gigaam-drive-selection', JSON.stringify(['tx-1']));
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_selection') return { local: [], remote: [] };
			if (command === 'google_drive_tree') return { snapshots: [], branches: [] };
			if (command === 'google_drive_restored_ids' || command === 'google_drive_local_lineage') return {};
			return backend(command, ...args);
		});
		render(Page);
		await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect((await screen.findByRole('checkbox', { name: /meeting\.wav/ }) as HTMLInputElement).checked).toBe(true);
	});
	it('does not report 100 percent while upload verification is incomplete', async () => {
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_tree') return { snapshots: [], branches: [] };
			if (command === 'google_drive_restored_ids' || command === 'google_drive_local_lineage') return {};
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		await fireEvent.click(await screen.findByRole('checkbox', { name: /meeting\.wav/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Отправить выбранное' }));
		await waitFor(() => expect(screen.queryByText(/100%/)).toBeNull());
	});
	it('rejects branch data from a different Drive listing', async () => {
		mockBackend({ items: [historyItem] }); const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation(async (command: string, ...args: unknown[]) => {
			if (command === 'google_drive_tree') return { snapshots: await invoke('google_drive_snapshots'), branches: await invoke('google_drive_branches') };
			if (command === 'google_drive_connected') return true;
			if (command === 'google_drive_snapshots') return [{ id: 'old', name: 'old.json' }];
			if (command === 'google_drive_branches') return [{ file_id: 'new', original_id: 'id', revision: 'rev', concurrent: false }];
			return backend(command, ...args);
		});
		render(Page); await fireEvent.click(await screen.findByRole('button', { name: 'Google Drive' }));
		expect((await screen.findAllByText(/Список Drive изменился/)).length).toBeGreaterThan(0);
		expect(screen.queryByText(/old\.json/)).toBeNull();
	});
	beforeEach(() => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		localStorage.clear();
		// Desktop tests run with the Tauri IPC bridge present.
		Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} });
		mockBackend();
	});

	afterEach(() => {
		cleanup();
	});

	it('shows the first-run model download and invokes the downloader', async () => {
		mockBackend({ installed: false });
		render(Page);

		const download = await screen.findByRole('button', { name: 'Загрузить' });
		expect(screen.getByText('Требуется модель распознавания')).toBeInTheDocument();
		await waitFor(() => expect(screen.getByText('217 MB')).toBeInTheDocument());
		await fireEvent.click(download);
		expect(invoke).toHaveBeenCalledWith('download_model');
	});

	it('switches the visible UI language', async () => {
		render(Page);
		await screen.findByPlaceholderText('Поиск');

		await fireEvent.click(screen.getByLabelText('Язык'));
		await fireEvent.click(screen.getByRole('button', { name: '🇬🇧 English' }));
		expect(screen.getByRole('tree', { name: 'Recordings and folders' })).toBeInTheDocument();
		expect(screen.getByPlaceholderText('Search')).toBeInTheDocument();
	});

	it('creates a folder through the desktop organization contract', async () => {
		render(Page);
		await screen.findByPlaceholderText('Поиск');
		await fireEvent.click(screen.getByRole('button', { name: 'Создать папку' }));
		await fireEvent.input(screen.getByRole('textbox', { name: 'Название папки' }), { target: { value: 'Folder' } });
		await fireEvent.click(screen.getByRole('button', { name: /^Создать$/ }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('save_organization', {
			nodes: [expect.objectContaining({ kind: 'folder', name: 'Folder', parent_id: null })], previous: []
		}));
		expect(await screen.findByRole('button', { name: '📁 Folder' })).toBeInTheDocument();
	});

	it('browser shell loads history without invoking Desktop IPC', async () => {
		Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
		const metadata = vi.spyOn(browserStorage, 'listBrowserMetadata').mockResolvedValue([]);
		const organization = vi.spyOn(browserStorage, 'loadBrowserOrganization').mockResolvedValue([]);
		try {
			render(Page);
			await screen.findByTestId('web-mode-banner');
			await waitFor(() => expect(metadata).toHaveBeenCalled());
			expect(invoke).not.toHaveBeenCalled();
		} finally { cleanup(); metadata.mockRestore(); organization.mockRestore(); }
	});

	it('renders history and loads a selected transcript', async () => {
		mockBackend({ items: [historyItem] });
		render(Page);

		await fireEvent.click(await screen.findByText('meeting.wav'));
		expect(await screen.findByText('Проверочный текст')).toBeInTheDocument();
		expect(invoke).toHaveBeenCalledWith('get_segments', { id: 'tx-1' });
	});

	it('renames a recording by double click in the tree', async () => {
		mockBackend({ items: [historyItem] });
		render(Page);
		await screen.findByText('meeting.wav');
		await fireEvent.dblClick(screen.getByTestId('recording-item'));
		await waitFor(() => expect(document.querySelector('input[data-place="list"]')).not.toBeNull());
	});

	it('passes explicitly selected Ogg format to native recording', async () => {
		mockBackend({ installed: true });
		render(Page);
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('model_status'));
		await fireEvent.click(await screen.findByRole('button', { name: /^Записать$/ }));
		await fireEvent.change(screen.getByRole('combobox', { name: 'Формат записи' }), { target: { value: 'ogg' } });
		await fireEvent.click(screen.getByRole('button', { name: /●\s*Запись/ }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('start_live', expect.objectContaining({ archiveFormat: 'ogg' })));
	});

	it('keeps a saved lossless preference instead of replacing it with compact default', async () => {
		localStorage.setItem('gigaam.archive-format', 'flac');
		mockBackend({ installed: true });
		try {
			render(Page);
			await fireEvent.click(await screen.findByRole('button', { name: /^Записать$/ }));
			expect((screen.getByRole('combobox', { name: 'Формат записи' }) as HTMLSelectElement).value).toBe('flac');
		} finally { localStorage.removeItem('gigaam.archive-format'); }
	});

	it('exports the selected transcript using the extension chosen in the save dialog', async () => {
		mockBackend({ items: [historyItem] });
		save.mockResolvedValue('C:\\exports\\meeting.srt');
		render(Page);

		await fireEvent.click(await screen.findByText('meeting.wav'));
		await screen.findByText('Проверочный текст');
		await fireEvent.click(screen.getByText('Экспорт', { selector: 'summary' }));
        await fireEvent.click(screen.getByRole('button', { name: 'В файл' }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('save_export', {
			id: 'tx-1',
			format: 'srt',
			path: 'C:\\exports\\meeting.srt'
		}));
	});

	it('imports selected media through the Tauri commands', async () => {
		open.mockResolvedValue('C:\\audio\\sample.wav');
		render(Page);
		const add = await screen.findByRole('button', { name: 'Открыть', hidden: false });
		await waitFor(() => expect(add).toBeEnabled());

		await fireEvent.click(add);
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('enqueue_files', { paths: ['C:\\audio\\sample.wav'] }));
		expect(open).toHaveBeenCalledWith(expect.objectContaining({ multiple: true, directory: false }));
	});

	it('starts live recording with the chosen source and archive format', async () => {
		render(Page);
		await waitFor(() => expect(screen.queryByText('Требуется модель распознавания')).not.toBeInTheDocument());
		await fireEvent.click(await screen.findByRole('button', { name: 'Записать' }));
		await screen.findByText('Тип источника');

		await fireEvent.click(screen.getByRole('button', { name: '●Запись' }));
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('start_live', expect.objectContaining({
			source: 'system_and_microphone',
			microphone: 'Test microphone',
			archiveFormat: 'ogg',
			micGain: 3
		})));
	});

	it('keeps the UI responsive while stop acknowledgement is pending', async () => {
		let resolveStop!: () => void;
		let stopping = false;
		const backend = invoke.getMockImplementation()!;
		invoke.mockImplementation((command: string, ...args: unknown[]) => {
			if (command === 'start_live') return Promise.resolve({ started_ms: Date.now() });
			if (command === 'live_status') return Promise.resolve({ microphone_level: 0, system_level: 0, model_ready: true, paused: false });
			if (command === 'stop_live') { stopping = true; return new Promise<void>((resolve) => { resolveStop = resolve; }); }
			if (command === 'live_finalizing') return Promise.resolve(stopping);
			return backend(command, ...args);
		});
		render(Page);
		await waitFor(() => expect(screen.queryByText('Требуется модель распознавания')).not.toBeInTheDocument());
		await fireEvent.click(await screen.findByRole('button', { name: 'Записать' }));
		await fireEvent.click(screen.getByRole('button', { name: '●Запись' }));
		const stop = await screen.findByRole('button', { name: /Стоп/ });
		await waitFor(() => expect(stop).toBeEnabled());
		await fireEvent.click(stop);
		await waitFor(() => expect(invoke).toHaveBeenCalledWith('stop_live'));
		expect(stop).toBeDisabled();
		// A pending native acknowledgement must not freeze unrelated UI controls.
		await fireEvent.click(screen.getByLabelText('Язык'));
		await fireEvent.click(screen.getByRole('button', { name: '🇬🇧 English' }));
		expect(screen.getByLabelText('Language').textContent).toContain('🇬🇧');
		resolveStop();
	});
});
