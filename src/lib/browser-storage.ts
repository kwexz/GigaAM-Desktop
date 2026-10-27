import type { AudioBlockEvent, SessionState, TranscriptChange, TranscriptSegmentEvent } from './contracts';

const DB_NAME = 'gigaam-desktop-browser-v1';
const DB_VERSION = 1;
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
		};
		request.onsuccess = () => resolve(request.result);
	});
}

function transaction<T>(store: string, mode: IDBTransactionMode, action: (objectStore: IDBObjectStore) => IDBRequest<T> | void): Promise<T | undefined> {
	return openDb().then((db) => new Promise((resolve, reject) => {
		const tx = db.transaction(store, mode);
		const request = action(tx.objectStore(store));
		tx.onerror = () => reject(tx.error ?? new Error('IndexedDB transaction failed'));
		tx.oncomplete = () => resolve(request && 'result' in request ? request.result : undefined);
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

export async function appendOpfsChunk(path: string, chunk: Blob): Promise<number> {
	const root = await opfsRoot();
	const handle = await root.getFileHandle(path, { create: true });
	const writable = await handle.createWritable({ keepExistingData: true });
	const existing = await handle.getFile();
	await writable.seek(existing.size);
	await writable.write(chunk);
	await writable.close();
	return existing.size + chunk.size;
}

export async function readOpfsArchive(path: string): Promise<File> {
	const root = await opfsRoot();
	return (await root.getFileHandle(path)).getFile();
}

export async function removeOpfsArchive(path: string): Promise<void> {
	const root = await opfsRoot();
	await root.removeEntry(path);
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

export async function putSegment(event: TranscriptSegmentEvent): Promise<void> {
	await transaction(SEGMENT_STORE, 'readwrite', (store) => store.put(event));
	const change: TranscriptChange = {
		change_id: event.idempotency_key,
		session_id: event.session_id,
		kind: 'segment_upsert',
		revision: event.revision,
		payload: event,
	};
	await transaction(CHANGE_STORE, 'readwrite', (store) => store.put(change));
}

export async function changesSince(session_id: string, revision: number): Promise<TranscriptChange[]> {
	const db = await openDb();
	return new Promise((resolve, reject) => {
		const request = db.transaction(CHANGE_STORE, 'readonly').objectStore(CHANGE_STORE).index('session_revision').getAll();
		request.onerror = () => reject(request.error ?? new Error('IndexedDB read failed'));
		request.onsuccess = () => resolve((request.result as TranscriptChange[]).filter((change) => change.session_id === session_id && change.revision > revision));
	});
}
