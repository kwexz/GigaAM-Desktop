import type { CloudProviderName } from './cloud-auth';

/** Provider API adapter. Remote URLs are fixed provider endpoints; download URLs
 * returned by Yandex are restricted before forwarding the request (no bearer on CDN). */
export class CloudFiles {
	constructor(readonly provider: CloudProviderName, private token: () => string, private request: typeof fetch = fetch) {}
	private async api(url: string, init: RequestInit = {}): Promise<Response> {
		const response = await this.request(url, { ...init, redirect: 'error', headers: { ...Object.fromEntries(new Headers(init.headers)), Authorization: `${this.provider === 'google-drive' ? 'Bearer' : 'OAuth'} ${this.token()}` } });
		if (!response.ok) throw new Error(`Cloud request failed (${response.status})`);
		return response;
	}
	private safeName(name: string): string {
		if (!/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,180}$/.test(name) || name.includes('..')) throw new Error('Invalid cloud object name');
		return name;
	}
	async put(name: string, content: Blob): Promise<void> {
		name = this.safeName(name);
		if (this.provider === 'yandex-disk') {
			const response = await this.api(`https://cloud-api.yandex.net/v1/disk/resources/upload?path=${encodeURIComponent('app:/' + name)}&overwrite=true`);
			const { href } = await response.json();
			const url = this.yandexTransferUrl(href);
			const upload = await this.request(url, { method: 'PUT', body: content, redirect: 'error' });
			if (!upload.ok) throw new Error(`Cloud upload failed (${upload.status})`);
			return;
		}
		const existing = await this.googleId(name);
		if (existing) {
			await this.api(`https://www.googleapis.com/upload/drive/v3/files/${encodeURIComponent(existing)}?uploadType=media`, { method: 'PATCH', body: content });
		} else {
			const boundary = `gigaam_${crypto.randomUUID()}`;
			const metadata = JSON.stringify({ name, parents: ['appDataFolder'] });
			const body = new Blob([`--${boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n${metadata}\r\n--${boundary}\r\nContent-Type: application/octet-stream\r\n\r\n`, content, `\r\n--${boundary}--`]);
			await this.api('https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart', { method: 'POST', headers: { 'Content-Type': `multipart/related; boundary=${boundary}` }, body });
		}
	}
	async get(name: string): Promise<Blob | undefined> {
		name = this.safeName(name);
		if (this.provider === 'google-drive') {
			const id = await this.googleId(name);
			if (!id) return;
			return (await this.api(`https://www.googleapis.com/drive/v3/files/${encodeURIComponent(id)}?alt=media`)).blob();
		}
		const response = await this.request(`https://cloud-api.yandex.net/v1/disk/resources/download?path=${encodeURIComponent('app:/' + name)}`, { redirect: 'error', headers: { Authorization: `OAuth ${this.token()}` } });
		if (response.status === 404) return;
		if (!response.ok) throw new Error(`Cloud download failed (${response.status})`);
		const { href } = await response.json();
		const download = await this.request(this.yandexTransferUrl(href), { redirect: 'error' });
		if (!download.ok) throw new Error(`Cloud download failed (${download.status})`);
		return download.blob();
	}
	async putVerified(name: string, content: Blob): Promise<string> {
		if (content.size > 16 * 1024 * 1024) throw new Error('Split large cloud objects into bounded chunks before verification');
		const hash = async (blob: Blob) => [...new Uint8Array(await crypto.subtle.digest('SHA-256', await blob.arrayBuffer()))].map((byte) => byte.toString(16).padStart(2, '0')).join('');
		const expected = await hash(content);
		await this.put(name, content);
		const stored = await this.get(name);
		if (!stored || stored.size !== content.size || await hash(stored) !== expected) throw new Error('Remote cloud integrity verification failed');
		return expected;
	}
	private async googleId(name: string): Promise<string | undefined> {
		const query = new URLSearchParams({ spaces: 'appDataFolder', q: `name = '${name}' and trashed = false`, fields: 'files(id)', pageSize: '2' });
		const response = await this.api(`https://www.googleapis.com/drive/v3/files?${query}`);
		const result = await response.json();
		if (!Array.isArray(result.files) || result.files.length > 1) throw new Error('Ambiguous cloud object');
		return result.files[0]?.id;
	}
	private yandexTransferUrl(value: unknown): string {
		if (typeof value !== 'string') throw new Error('Invalid cloud transfer URL');
		const url = new URL(value);
		if (url.protocol !== 'https:' || url.username || url.password || (url.port && url.port !== '443') || !(url.hostname.endsWith('.yandex.net') || url.hostname.endsWith('.yandex.ru'))) throw new Error('Unsafe cloud transfer URL');
		return url.href;
	}
}
