import { CloudFiles } from './cloud-files';

const CHUNK_BYTES = 4 * 1024 * 1024;
export type CloudBackupManifest = {
	version: 1;
	id: string;
	bytes: number;
	mime: string;
	chunks: Array<{ name: string; bytes: number; sha256: string }>;
};

/** Content-addressed bounded uploads. The manifest is committed last, after
 * every chunk has been read back and verified. Local audio is never deleted. */
export async function uploadCloudArchive(provider: CloudFiles, id: string, audio: Blob, progress?: (saved: number, total: number) => void): Promise<CloudBackupManifest> {
	if (!/^[a-zA-Z0-9-]{1,100}$/.test(id)) throw new Error('Invalid session ID');
	const manifest: CloudBackupManifest = { version: 1, id, bytes: audio.size, mime: audio.type, chunks: [] };
	for (let offset = 0; offset < audio.size; offset += CHUNK_BYTES) {
		const chunk = audio.slice(offset, offset + CHUNK_BYTES);
		const sha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256', await chunk.arrayBuffer()))].map((byte) => byte.toString(16).padStart(2, '0')).join('');
		const name = `${id}-${manifest.chunks.length}-${sha256}.chunk`;
		await provider.putVerified(name, chunk);
		manifest.chunks.push({ name, bytes: chunk.size, sha256 });
		progress?.(Math.min(offset + chunk.size, audio.size), audio.size);
	}
	await provider.putVerified(`${id}.manifest.json`, new Blob([JSON.stringify(manifest)], { type: 'application/json' }));
	return manifest;
}

export async function downloadCloudArchive(provider: CloudFiles, id: string, destination: WritableStream<Uint8Array>): Promise<void> {
	if (!/^[a-zA-Z0-9-]{1,100}$/.test(id)) throw new Error('Invalid session ID');
	const file = await provider.get(`${id}.manifest.json`);
	if (!file || file.size > 1024 * 1024) throw new Error('Missing or oversized cloud manifest');
	const manifest = JSON.parse(await file.text()) as CloudBackupManifest;
	if (manifest.version !== 1 || manifest.id !== id || !Array.isArray(manifest.chunks) || !Number.isSafeInteger(manifest.bytes) || manifest.bytes < 0) throw new Error('Invalid cloud manifest');
	const writer = destination.getWriter();
	try {
		let total = 0;
		for (const entry of manifest.chunks) {
			if (!Number.isSafeInteger(entry.bytes) || entry.bytes < 1 || entry.bytes > CHUNK_BYTES || !/^[a-f0-9]{64}$/.test(entry.sha256) || !entry.name.startsWith(`${id}-`)) throw new Error('Invalid cloud chunk');
			const chunk = await provider.get(entry.name);
			if (!chunk || chunk.size !== entry.bytes) throw new Error('Missing cloud chunk');
			const data = await chunk.arrayBuffer();
			const hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256', data))].map((byte) => byte.toString(16).padStart(2, '0')).join('');
			if (hash !== entry.sha256) throw new Error('Cloud chunk checksum mismatch');
			await writer.write(new Uint8Array(data));
			total += chunk.size;
		}
		if (total !== manifest.bytes) throw new Error('Cloud archive size mismatch');
		await writer.close();
	} catch (error) { await writer.abort(error); throw error; }
	finally { writer.releaseLock(); }
}
