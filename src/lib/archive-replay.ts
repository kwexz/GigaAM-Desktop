import type { AudioBlockEvent } from './contracts';
import { archivedBlocks, listBrowserMetadata, readOpfsArchive } from './browser-storage';
import type { RemoteEngine } from './remote-engine';

export async function replayToRemote(sessionId: string, engine: RemoteEngine, signal?: AbortSignal): Promise<void> {
	for await (const block of replayArchivedBlocks(sessionId)) {
		if (signal?.aborted) throw new Error('Replay cancelled');
		if (block.seq <= engine.durableSequence(block.track_id)) continue;
		await engine.waitForCapacity(signal);
		await engine.enqueue(block);
	}
	await engine.waitForDrain(signal);
}

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
