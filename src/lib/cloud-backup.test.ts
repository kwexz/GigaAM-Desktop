import { describe, expect, it, vi } from 'vitest';
import { CloudFiles } from './cloud-files';
import { uploadCloudArchive, downloadCloudArchive } from './cloud-backup';

describe('bounded cloud archive transfer', () => {
	it('writes verified chunks before manifest and restores exact bytes', async () => {
		const provider = new CloudFiles('google-drive', () => 'test');
		const objects = new Map<string, Blob>();
		vi.spyOn(provider, 'put').mockImplementation(async (name, content) => { objects.set(name, content); });
		vi.spyOn(provider, 'get').mockImplementation(async (name) => objects.get(name));
		const input = new Blob(['recording-audio']);
		await uploadCloudArchive(provider, 'session', input);
		expect([...objects.keys()].at(-1)).toBe('session.manifest.json');
		const output: Uint8Array[] = [];
		await downloadCloudArchive(provider, 'session', new WritableStream({ write(chunk) { output.push(chunk); } }));
		expect(new TextDecoder().decode(output[0])).toBe('recording-audio');
	});
	it('never commits manifest after failed chunk verification', async () => {
		const provider = new CloudFiles('yandex-disk', () => 'test');
		const put = vi.spyOn(provider, 'putVerified').mockRejectedValue(new Error('network failure'));
		await expect(uploadCloudArchive(provider, 'session', new Blob(['audio']))).rejects.toThrow('network');
		expect(put).toHaveBeenCalledOnce();
		expect(put.mock.calls[0][0]).toMatch(/\.chunk$/);
	});
});
