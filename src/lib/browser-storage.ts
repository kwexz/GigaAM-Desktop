import type { AudioBlockEvent, SessionState, TranscriptChange, TranscriptSegmentEvent } from './contracts';
import { validateOrganization, type OrganizationNode } from './organization';

const DB_NAME = 'gigaam-desktop-browser-v1';
const DB_VERSION = 2;
const META_STORE = 'metadata';
const SEGMENT_STORE = 'segments';
const CHANGE_STORE = 'changes';

export type BrowserArchiveMeta = {
	session_id: string;
	track_id: AudioBlockEvent['track_id'];
	path: string;
	mime_type: string;
	bytes: number;
	block_count: number;
	content_hash?: string;
	created_ms: number;
};

function openDb(): Promise<IDBDatabase> {
	return new Promise((resolve, reject) => {
		const request = indexedDB.open(DB_NAME, DB_VERSION);
		request.onerror = () => reject(request.error ?? new Error('IndexedDB open failed'));
		request.onupgradeneeded = () => {
			const db = request.result;
			if (!db.objectStoreNames.contains(META_STORE)) db.createObjectStore(META_STORE, { keyPath: 'key' });
			if (!db.objectStoreNames.contains(SEGMENT_STORE)) db.createObjectStore(SEGMENT_STORE, { keyPath: ['session_id', 'segment_id'] });
			if (!db.objectStoreNames.contains(CHANGE_STORE)) {
				const changes = db.createObjectStore(CHANGE_STORE, { keyPath: 'change_id' });
				changes.createIndex('session_revision', ['session_id', 'revision']);
			}
			// Organization is additive metadata: existing audio paths/segment keys
			// are untouched; missing entries are placed in the logical root.
			request.transaction!.objectStore(META_STORE).put({ key: 'schema:organization', version: 1 });
		};
		request.onblocked = () => reject(new Error('Close older GigaAM tabs to upgrade local storage'));
		request.onsuccess = () => { request.result.onversionchange = () => request.result.close(); resolve(request.result); };
	});
}

function transaction<T>(store: string, mode: IDBTransactionMode, action: (objectStore: IDBObjectStore) => IDBRequest<T> | void): Promise<T | undefined> {
	return openDb().then((db) => new Promise((resolve, reject) => {
		const tx = db.transaction(store, mode);
		const request = action(tx.objectStore(store));
		tx.onerror = tx.onabort = () => { db.close(); reject(tx.error ?? new Error('IndexedDB transaction failed')); };
		tx.oncomplete = () => { db.close(); resolve(request && 'result' in request ? request.result : undefined); };
	}));
}

export async function requestPersistentStorage(): Promise<boolean> {
	if (!navigator.storage?.persist) return false;
	return navigator.storage.persist();
}

async function opfsRoot(): Promise<FileSystemDirectoryHandle> {
	if (!navigator.storage?.getDirectory) throw new Error('OPFS is unavailable in this browser');
	return navigator.storage.getDirectory();
}

async function archiveHandle(path: string, create = false): Promise<FileSystemFileHandle> {
	const parts = path.split('/');
	if (parts.some((part) => !part || part === '.' || part === '..')) throw new Error('Invalid archive path');
	let directory = await opfsRoot();
	for (const part of parts.slice(0, -1)) directory = await directory.getDirectoryHandle(part, { create });
	return directory.getFileHandle(parts[parts.length - 1], { create });
}

export async function appendOpfsChunk(path: string, chunk: Blob): Promise<number> {
	const handle = await archiveHandle(path, true);
	const writable = await handle.createWritable({ keepExistingData: true });
	const existing = await handle.getFile();
	await writable.seek(existing.size);
	await writable.write(chunk);
	await writable.close();
	return existing.size + chunk.size;
}

export async function readOpfsArchive(path: string): Promise<File> {
	return (await archiveHandle(path)).getFile();
}

export async function removeOpfsArchive(path: string): Promise<void> {
	const parts = path.split('/');
	if (parts.some((part) => !part || part === '.' || part === '..')) throw new Error('Invalid archive path');
	let directory = await opfsRoot();
	for (const part of parts.slice(0, -1)) directory = await directory.getDirectoryHandle(part);
	await directory.removeEntry(parts[parts.length - 1]);
}

export async function listBrowserMetadata(): Promise<Array<Record<string, unknown>>> {
	return (await transaction(META_STORE, 'readonly', (store) => store.getAll()) ?? []) as Array<Record<string, unknown>>;
}

export async function loadBrowserOrganization(): Promise<OrganizationNode[]> {
	const entry = await transaction(META_STORE, 'readonly', (store) => store.get('organization:v1'));
	return (entry as { nodes?: OrganizationNode[] } | undefined)?.nodes ?? [];
}

export async function saveBrowserOrganization(nodes: OrganizationNode[]): Promise<void> {
	validateOrganization(nodes);
	const snapshot = JSON.parse(JSON.stringify(nodes)) as OrganizationNode[];
	await transaction(META_STORE, 'readwrite', (store) => store.put({ key: 'organization:v1', nodes: snapshot }));
}

export async function mergeBrowserOrganization(previous: OrganizationNode[], proposed: OrganizationNode[]): Promise<OrganizationNode[]> {
	return updateBrowserOrganization((latest) => {
		const removed = new Set(previous.filter((node) => !proposed.some((entry) => entry.id === node.id)).map((node) => node.id));
		for (const id of removed) {
			const before = previous.find((node) => node.id === id);
			const current = latest.find((node) => node.id === id);
			if (current && JSON.stringify(before) !== JSON.stringify(current)) throw new Error('Folder changed in another tab; reload and retry');
		}
		const changed = proposed.filter((node) => JSON.stringify(node) !== JSON.stringify(previous.find((entry) => entry.id === node.id)));
		const result = latest.filter((node) => !removed.has(node.id)).map((node) => ({ ...node }));
		for (const node of proposed) if (!result.some((entry) => entry.id === node.id) && !changed.some((entry) => entry.id === node.id)) result.push({ ...node });
		for (const node of changed) {
			const index = result.findIndex((entry) => entry.id === node.id);
			const old = previous.find((entry) => entry.id === node.id);
			if (old && index >= 0 && JSON.stringify(result[index]) !== JSON.stringify(old)) throw new Error('Organization changed in another tab; reload and retry');
			if (index >= 0) result[index] = { ...node }; else result.push({ ...node });
		}
		// Reject parent deletion races instead of silently orphaning concurrent data.
		for (const node of result) {
			let parent = node.parent_id;
			const seen = new Set([node.id]);
			while (parent !== null) {
				if (seen.has(parent)) throw new Error('Organization cycle');
				seen.add(parent);
				const folder = result.find((entry) => entry.id === parent && entry.kind === 'folder');
				if (!folder) throw new Error('Parent changed in another tab');
				parent = folder.parent_id;
			}
		}
		return result;
	});
}

export async function updateBrowserOrganization(update: (nodes: OrganizationNode[]) => OrganizationNode[]): Promise<OrganizationNode[]> {
	const db = await openDb();
	return new Promise((resolve, reject) => {
		const tx = db.transaction([META_STORE, CHANGE_STORE], 'readwrite');
		const store = tx.objectStore(META_STORE);
		let next: OrganizationNode[] = [];
		let failure: unknown;
		store.get('organization:v1').onsuccess = (event) => {
			try {
				const value = (event.target as IDBRequest).result as { nodes?: OrganizationNode[] } | undefined;
				next = update(value?.nodes ?? []);
				validateOrganization(next);
				store.put({ key: 'organization:v1', nodes: JSON.parse(JSON.stringify(next)) });
				const change: TranscriptChange = { change_id: crypto.randomUUID(), session_id: 'organization', kind: 'session_update', revision: Date.now(), payload: { version: 1, nodes: JSON.parse(JSON.stringify(next)) } };
				tx.objectStore(CHANGE_STORE).put(change);
			} catch (error) { failure = error; tx.abort(); }
		};
		tx.oncomplete = () => { db.close(); resolve(next); };
		tx.onerror = tx.onabort = () => { db.close(); reject(failure ?? tx.error ?? new Error('Organization transaction failed')); };
	});
}

export async function putBrowserRecording(record: { id: string; name: string; created_ms: number; duration_seconds: number }): Promise<void> {
	await transaction(META_STORE, 'readwrite', (store) => store.put({ key: `recording:${record.id}`, ...record }));
}

export async function renameBrowserRecording(id: string, name: string): Promise<void> {
	name = name.trim();
	if (!name || name.length > 1024) throw new Error('Invalid recording name');
	const db = await openDb();
	await new Promise<void>((resolve, reject) => {
		const tx = db.transaction(META_STORE, 'readwrite');
		const store = tx.objectStore(META_STORE);
		const request = store.get(`recording:${id}`);
		request.onsuccess = () => { if (!request.result) { tx.abort(); return; } store.put({ ...request.result, name }); };
		tx.oncomplete = () => { db.close(); resolve(); };
		tx.onerror = tx.onabort = () => { db.close(); reject(tx.error ?? new Error('Recording rename failed')); };
	});
}

export async function editBrowserSegment(sessionId: string, segmentId: string, text: string): Promise<void> {
	const segment = (await browserSegments(sessionId)).find((entry) => entry.segment_id === segmentId);
	if (!segment || !text.trim()) throw new Error('Unknown or empty segment');
	await putSegment({ ...segment, text, revision: segment.revision + 1, idempotency_key: crypto.randomUUID() });
}

/** Metadata is removed only after audio deletion succeeds. Interrupted cleanup
 * leaves a visible retryable record, never a false successful deletion. */
export async function deleteBrowserRecording(sessionId: string): Promise<void> {
	if (navigator.locks) return navigator.locks.request(`gigaam-recording:${sessionId}`, { ifAvailable: true }, (lock) => {
		if (!lock) throw new Error('Cannot delete a recording active in another tab');
		return deleteBrowserRecordingUnlocked(sessionId);
	});
	return deleteBrowserRecordingUnlocked(sessionId);
}

async function deleteBrowserRecordingUnlocked(sessionId: string): Promise<void> {
	const metadata = await listBrowserMetadata();
	for (const archive of metadata.filter((entry) => entry.session_id === sessionId && String(entry.key).startsWith('archive:'))) {
		try { await removeOpfsArchive(String(archive.path)); }
		catch (error) { if (!(error instanceof DOMException && error.name === 'NotFoundError')) throw error; }
	}
	const db = await openDb();
	await new Promise<void>((resolve, reject) => {
		const tx = db.transaction([META_STORE, SEGMENT_STORE, CHANGE_STORE], 'readwrite');
		const meta = tx.objectStore(META_STORE);
		for (const entry of metadata) if (entry.session_id === sessionId || entry.key === `recording:${sessionId}`) meta.delete(String(entry.key));
		const organization = metadata.find((entry) => entry.key === 'organization:v1');
		if (organization) meta.put({ ...organization, nodes: (organization.nodes as OrganizationNode[]).filter((node) => node.id !== sessionId) });
		for (const name of [SEGMENT_STORE, CHANGE_STORE]) {
			const cursor = tx.objectStore(name).openCursor();
			cursor.onsuccess = () => { const row = cursor.result; if (!row) return; if (row.value.session_id === sessionId) row.delete(); row.continue(); };
		}
		tx.oncomplete = () => { db.close(); resolve(); };
		tx.onerror = tx.onabort = () => { db.close(); reject(tx.error ?? new Error('Recording deletion failed')); };
	});
}

export type BrowserCloudJob = {
	id: string;
	provider: 'google-drive' | 'yandex-disk';
	session_id: string;
	archive_path: string;
	state: 'pending' | 'syncing' | 'synced' | 'failed';
	attempts: number;
};

export async function putBrowserCloudJob(job: BrowserCloudJob): Promise<void> {
	await transaction(META_STORE, 'readwrite', (store) => store.put({ key: `cloud-job:${job.id}`, ...job }));
}

export async function browserCloudJobs(): Promise<BrowserCloudJob[]> {
	return (await listBrowserMetadata()).filter((entry) => String(entry.key).startsWith('cloud-job:')) as unknown as BrowserCloudJob[];
}

export async function putArchiveMeta(meta: BrowserArchiveMeta): Promise<void> {
	await transaction(META_STORE, 'readwrite', (store) => store.put({ key: `archive:${meta.session_id}:${meta.track_id}`, ...meta }));
}

export async function putSessionState(session_id: string, state: SessionState): Promise<void> {
	await transaction(META_STORE, 'readwrite', (store) => store.put({ key: `session:${session_id}`, session_id, state }));
}

export async function getSessionState(session_id: string): Promise<SessionState | undefined> {
	const value = await transaction(META_STORE, 'readonly', (store) => store.get(`session:${session_id}`));
	return (value as { state?: SessionState } | undefined)?.state;
}

export async function browserSegments(sessionId: string): Promise<TranscriptSegmentEvent[]> {
	const rows = await transaction(SEGMENT_STORE, 'readonly', (store) => store.getAll()) as TranscriptSegmentEvent[];
	return rows.filter((row) => row.session_id === sessionId).sort((a, b) => a.start_seconds - b.start_seconds);
}

/** Recover UI state from durable session FSM, not an invented duration/status. */
export async function browserRecordingStatus(sessionId: string): Promise<'completed' | 'interrupted' | 'failed'> {
	const state = await getSessionState(sessionId);
	if (state?.recording === 'failed') return 'failed';
	if (state?.recording === 'completed') return 'completed';
	return 'interrupted';
}


export async function putSegment(event: TranscriptSegmentEvent): Promise<void> {
	const change: TranscriptChange = {
		change_id: event.idempotency_key,
		session_id: event.session_id,
		kind: 'segment_upsert',
		revision: event.revision,
		payload: event,
	};
	const db = await openDb();
	await new Promise<void>((resolve, reject) => {
		const tx = db.transaction([SEGMENT_STORE, CHANGE_STORE], 'readwrite');
		const segments = tx.objectStore(SEGMENT_STORE);
		const request = segments.get([event.session_id, event.segment_id]);
		request.onsuccess = () => {
			const previous = request.result as TranscriptSegmentEvent | undefined;
			if (previous && previous.revision >= event.revision) return;
			segments.put(event);
			tx.objectStore(CHANGE_STORE).put(change);
		};
		tx.oncomplete = () => { db.close(); resolve(); };
		tx.onerror = tx.onabort = () => { db.close(); reject(tx.error ?? new Error('Transcript transaction failed')); };
	});
}

export async function changesSince(session_id: string, revision: number): Promise<TranscriptChange[]> {
	const changes = await transaction(CHANGE_STORE, 'readonly', (store) => store.index('session_revision').getAll()) as TranscriptChange[];
	return changes.filter((change) => change.session_id === session_id && change.revision > revision);
}
