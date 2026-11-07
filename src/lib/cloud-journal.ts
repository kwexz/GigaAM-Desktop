import type { TranscriptChange } from './contracts';
import { CloudFiles } from './cloud-files';

export type JournalSnapshot = { version: 1; device_id: string; changes: TranscriptChange[] };

/** Immutable change IDs make replay idempotent. Conflicting same-ID revisions
 * with different payloads are retained as explicit conflicts, never overwritten. */
export function mergeJournal(local: TranscriptChange[], incoming: TranscriptChange[]): { changes: TranscriptChange[]; conflicts: TranscriptChange[] } {
	const changes = new Map(local.map((change) => [change.change_id, change]));
	const conflicts: TranscriptChange[] = [];
	for (const change of incoming) {
		const previous = changes.get(change.change_id);
		if (!previous) { changes.set(change.change_id, change); continue; }
		if (JSON.stringify(previous.payload) !== JSON.stringify(change.payload)) {
			conflicts.push(change);
			continue;
		}
		if (change.revision > previous.revision) changes.set(change.change_id, change);
	}
	return { changes: [...changes.values()].sort((a, b) => a.revision - b.revision || a.change_id.localeCompare(b.change_id)), conflicts };
}

export async function pushJournal(provider: CloudFiles, snapshot: JournalSnapshot): Promise<void> {
	if (!/^[a-zA-Z0-9-]{1,100}$/.test(snapshot.device_id)) throw new Error('Invalid device ID');
	if (snapshot.version !== 1 || snapshot.changes.length > 10_000) throw new Error('Invalid journal snapshot');
	await provider.putVerified(`journal-${snapshot.device_id}.json`, new Blob([JSON.stringify(snapshot)], { type: 'application/json' }));
}

export async function pullJournal(provider: CloudFiles, deviceId: string): Promise<TranscriptChange[]> {
	if (!/^[a-zA-Z0-9-]{1,100}$/.test(deviceId)) throw new Error('Invalid device ID');
	const object = await provider.get(`journal-${deviceId}.json`);
	if (!object) return [];
	if (object.size > 16 * 1024 * 1024) throw new Error('Oversized cloud journal');
	const snapshot = JSON.parse(await object.text()) as JournalSnapshot;
	if (snapshot.version !== 1 || snapshot.device_id !== deviceId || !Array.isArray(snapshot.changes) || snapshot.changes.length > 10_000) throw new Error('Invalid cloud journal');
	for (const change of snapshot.changes) {
		if (typeof change.change_id !== 'string' || typeof change.session_id !== 'string' || !Number.isSafeInteger(change.revision) || change.revision < 0 || !['segment_upsert', 'segment_delete', 'session_update'].includes(change.kind)) throw new Error('Invalid cloud change');
	}
	return snapshot.changes;
}
