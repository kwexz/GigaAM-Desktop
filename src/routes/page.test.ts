import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Page from './+page.svelte';

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
	beforeEach(() => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		localStorage.clear();
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
		await screen.findByText('Записи');

		await fireEvent.click(screen.getByRole('button', { name: 'RU' }));
		expect(screen.getByText('Recordings')).toBeInTheDocument();
		expect(screen.getByPlaceholderText('Search')).toBeInTheDocument();
	});

	it('renders history and loads a selected transcript', async () => {
		mockBackend({ items: [historyItem] });
		render(Page);

		await fireEvent.click(await screen.findByText('meeting.wav'));
		expect(await screen.findByText('Проверочный текст')).toBeInTheDocument();
		expect(invoke).toHaveBeenCalledWith('get_segments', { id: 'tx-1' });
	});

	it('exports the selected transcript using the extension chosen in the save dialog', async () => {
		mockBackend({ items: [historyItem] });
		save.mockResolvedValue('C:\\exports\\meeting.srt');
		render(Page);

		await fireEvent.click(await screen.findByText('meeting.wav'));
		await screen.findByText('Проверочный текст');
		await fireEvent.click(screen.getByRole('button', { name: 'Экспорт' }));
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
			archiveFormat: 'flac',
			micGain: 3
		})));
	});
});
