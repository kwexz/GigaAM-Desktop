import { afterEach, describe, expect, it, vi } from 'vitest';
import { BrowserCloudOutbox } from './cloud-outbox';
import { CloudFiles } from './cloud-files';
import * as storage from './browser-storage';
import * as backup from './cloud-backup';

describe('durable browser cloud outbox', () => {
	afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });
	it('keeps failed work durable and retries interrupted uploads', async () => {
		vi.stubGlobal('navigator', { locks: { request: async (_name: string, callback: () => Promise<void>) => callback() } });
		let job: storage.BrowserCloudJob = { id: 'job', provider: 'google-drive', session_id: 'session', archive_path: 'sessions/session/microphone.webm', state: 'syncing', attempts: 1 };
		vi.spyOn(storage, 'browserCloudJobs').mockImplementation(async () => [job]);
		vi.spyOn(storage, 'putBrowserCloudJob').mockImplementation(async (next) => { job = next; });
		vi.spyOn(storage, 'readOpfsArchive').mockResolvedValue(new File(['audio'], 'microphone.webm'));
		const upload = vi.spyOn(backup, 'uploadCloudArchive').mockRejectedValueOnce(new Error('network')).mockResolvedValue({ version: 1, id: 'session', bytes: 5, mime: '', chunks: [] });
		const outbox = new BrowserCloudOutbox();
		const provider = new CloudFiles('google-drive', () => 'token');
		await expect(outbox.drain(provider)).rejects.toThrow('network');
		expect(job.state).toBe('failed');
		await outbox.drain(provider);
		expect(job.state).toBe('synced');
		expect(job.attempts).toBe(3);
		expect(upload).toHaveBeenCalledTimes(2);
	});
});
