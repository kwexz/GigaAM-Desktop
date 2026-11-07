import { describe, expect, it, vi } from 'vitest';
import { CloudFiles } from './cloud-files';

describe('cloud object adapters', () => {
	it('does not acknowledge uploaded data when remote readback differs', async () => {
		const cloud = new CloudFiles('google-drive', () => 'token');
		vi.spyOn(cloud, 'put').mockResolvedValue();
		vi.spyOn(cloud, 'get').mockResolvedValue(new Blob(['different']));
		await expect(cloud.putVerified('audio', new Blob(['original']))).rejects.toThrow('integrity');
	});
	it('updates existing Google objects rather than duplicating retries', async () => {
		const request = vi.fn<typeof fetch>()
			.mockResolvedValueOnce(Response.json({ files: [{ id: 'file-id' }] }))
			.mockResolvedValueOnce(new Response('{}'));
		await new CloudFiles('google-drive', () => 'token', request).put('audio.webm', new Blob(['audio']));
		expect(request.mock.calls[1][1]?.method).toBe('PATCH');
		expect(new Headers(request.mock.calls[1][1]?.headers).get('Authorization')).toBe('Bearer token');
	});
	it('does not forward authorization to Yandex transfer hosts', async () => {
		const request = vi.fn<typeof fetch>()
			.mockResolvedValueOnce(Response.json({ href: 'https://upload.yandex.net/upload' }))
			.mockResolvedValueOnce(new Response());
		await new CloudFiles('yandex-disk', () => 'token', request).put('audio.ogg', new Blob(['audio']));
		expect(new Headers(request.mock.calls[1][1]?.headers).has('Authorization')).toBe(false);
	});
	it('rejects traversal and untrusted transfer destinations', async () => {
		const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json({ href: 'http://127.0.0.1/private' }));
		const cloud = new CloudFiles('yandex-disk', () => 'token', request);
		await expect(cloud.put('../audio', new Blob())).rejects.toThrow('name');
		await expect(cloud.put('audio', new Blob())).rejects.toThrow('Unsafe');
		expect(request).toHaveBeenCalledOnce();
	});
});
