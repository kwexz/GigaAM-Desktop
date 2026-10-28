import type { AudioBlockEvent } from './contracts';
import { archivedBlocks, listBrowserMetadata, readOpfsArchive } from './browser-storage';

/** Reconstruct bounded payloads from durable OPFS ranges, not a RAM backlog. */
export async function* replayArchivedBlocks(sessionId: string): AsyncGenerator<AudioBlockEvent> {
	const metadata = await listBrowserMetadata();
	for (const block of await archivedBlocks(sessionId)) {
		const archive = metadata.find((entry) => entry.session_id === sessionId && entry.track_id === block.event.track_id && String(entry.key).startsWith('archive:'));
		if (!archive || block.bytes < 1 || block.bytes > 16 * 1024 * 1024 || block.offset < 0) throw new Error('Invalid durable replay range');
		const file = await readOpfsArchive(String(archive.path));
		if (block.offset + block.bytes > file.size) throw new Error('Truncated durable replay archive');
		yield { ...block.event, payload: new Uint8Array(await file.slice(block.offset, block.offset + block.bytes).arrayBuffer()) };
	}
}
