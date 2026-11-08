import type { SyncProvider, TranscriptChange } from './contracts';

export type SyncRecord = TranscriptChange & { content_hash?: string; deleted?: boolean };
export type SyncCursor = { provider: string; value: string };

export interface CloudStorageProvider extends SyncProvider {
	readonly name: 'google-drive' | 'yandex-disk' | 'mock';
	logout(): Promise<void>;
}

export class MockCloudProvider implements CloudStorageProvider {
	readonly name = 'mock' as const;
	private records = new Map<string, SyncRecord>();
	private journal: Array<{ cursor: number; change: SyncRecord }> = [];
	private cursor = 0;
	private loggedOut = false;
	async changesSince(cursor: string | null): Promise<{ cursor: string; changes: TranscriptChange[] }> {
		if (this.loggedOut) throw new Error('provider logged out');
		const from = Number(cursor ?? 0);
		return { cursor: String(this.cursor), changes: this.journal.filter((entry) => entry.cursor > from).map((entry) => entry.change) };
	}
	async applyChange(change: TranscriptChange): Promise<void> {
		if (this.loggedOut) throw new Error('provider logged out');
		const previous = this.records.get(change.change_id);
		if (previous && previous.revision >= change.revision) return;
		this.records.set(change.change_id, change);
		this.cursor += 1;
		this.journal.push({ cursor: this.cursor, change });
	}
	async logout(): Promise<void> { this.loggedOut = true; }
}

/** Logical journal sync. It never uploads a SQLite file and is idempotent on
 * repeated pushes/pulls. Different segment IDs merge; same IDs use revision. */
export class SyncEngine {
	private readonly local = new Map<string, SyncRecord>();
	private cursor = '0';
	constructor(private readonly provider: CloudStorageProvider) {}
	async record(change: SyncRecord): Promise<void> {
		const previous = this.local.get(change.change_id);
		if (!previous || previous.revision < change.revision) this.local.set(change.change_id, change);
	}
	async push(): Promise<void> {
		for (const change of this.local.values()) await this.provider.applyChange(change);
	}
	async pull(): Promise<number> {
		const result = await this.provider.changesSince(this.cursor);
		for (const change of result.changes) {
			const previous = this.local.get(change.change_id);
			if (!previous || previous.revision < change.revision) this.local.set(change.change_id, change as SyncRecord);
		}
		this.cursor = result.cursor;
		return result.changes.length;
	}
	get(changeId: string): SyncRecord | undefined { return this.local.get(changeId); }
}
