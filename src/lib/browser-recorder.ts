import type { ArchiveWriter, AudioBlockEvent, CaptureSource, RecognitionEngine, TranscriptRepository } from './contracts';
import { assertAudioBlock, assertSegment } from './contracts';
import { browserRecordingMime } from './recording-format';
import { archivedBlocks, commitArchivedBlock } from './browser-storage';
import { appendOpfsChunk, changesSince, getSessionState, putArchiveMeta, putSegment, putSessionState, requestPersistentStorage, listBrowserMetadata, readOpfsArchive } from './browser-storage';

export type BrowserCaptureStatus = { microphone: boolean; system: boolean; systemAudioUnavailable: boolean };

export class BrowserCaptureSource implements CaptureSource {
	private listeners = new Set<(event: AudioBlockEvent) => void>();
	private recorders = new Map<AudioBlockEvent['track_id'], MediaRecorder>();
	private sequence = new Map<AudioBlockEvent['track_id'], number>();
	private started = new Map<AudioBlockEvent['track_id'], number>();
	private origin = 0;
	private conversion = new Map<AudioBlockEvent['track_id'], Promise<void>>();
	private lastEventAt = new Map<AudioBlockEvent['track_id'], number>();
	private streams: MediaStream[] = [];
	private pending = new Set<Promise<void>>();
	private pendingBytes = 0;
	private failure?: Error;
	private meterContext?: AudioContext;
	private meters = new Map<string, AnalyserNode>();
	levels(): { microphone: number; system: number } {
		const level = (track: string) => {
			const meter = this.meters.get(track); if (!meter) return 0;
			const values = new Float32Array(meter.fftSize); meter.getFloatTimeDomainData(values);
			return Math.sqrt(values.reduce((sum, value) => sum + value * value, 0) / values.length);
		};
		return { microphone: level('microphone'), system: level('system') };
	}
	private readonly errors = new Set<(error: Error) => void>();
	onError(listener: (error: Error) => void): () => void { this.errors.add(listener); return () => this.errors.delete(listener); }
	private report(error: Error): void { this.failure = error; for (const listener of this.errors) listener(error); }
	status: BrowserCaptureStatus = { microphone: false, system: false, systemAudioUnavailable: false };

	async start(sessionId: string, source: 'microphone' | 'system' | 'system_and_microphone' = 'system_and_microphone'): Promise<void> {
		this.origin = performance.now();
		// Invoke display capture before any awaited permission/storage request:
		// browsers require transient user activation for the chooser.
		const displayRequest = source !== 'microphone'
			? navigator.mediaDevices.getDisplayMedia({ video: true, audio: true, systemAudio: 'include' } as DisplayMediaStreamOptions).then((stream) => ({ stream }), () => ({ stream: undefined }))
			: undefined;
		void requestPersistentStorage().catch(() => false);
		if (source !== 'system') {
			const mic = await navigator.mediaDevices.getUserMedia({ audio: true });
			this.streams.push(mic);
			this.startTrack(sessionId, 'microphone', mic);
			this.status.microphone = true;
		}
		if (source === 'microphone') return;
		try {
			const display = (await displayRequest)?.stream;
			if (!display) throw new Error('Display capture was not granted');
			this.streams.push(display);
			const audio = display.getAudioTracks()[0];
			if (audio) {
				this.startTrack(sessionId, 'system', new MediaStream([audio]));
				this.status.system = true;
			} else {
				this.status.systemAudioUnavailable = true;
				display.getTracks().forEach((track) => track.stop());
			}
		} catch {
			this.status.systemAudioUnavailable = true;
		}
		if (source === 'system' && !this.status.system) throw new Error('System audio is unavailable; select microphone explicitly');
	}

	private startTrack(sessionId: string, trackId: AudioBlockEvent['track_id'], stream: MediaStream): void {
		this.meterContext ??= new AudioContext();
		void this.meterContext.resume().catch(() => {});
		const meter = this.meterContext.createAnalyser(); meter.fftSize = 1024;
		this.meterContext.createMediaStreamSource(stream).connect(meter); this.meters.set(trackId, meter);
		const requestedMime = browserRecordingMime((candidate) => MediaRecorder.isTypeSupported(candidate));
		const recorder = new MediaRecorder(stream, { mimeType: requestedMime });
		const mime = recorder.mimeType || requestedMime;
		recorder.onerror = () => { this.report(new Error('Browser recorder failed')); void this.stop().catch(() => {}); };
		for (const track of stream.getAudioTracks()) track.addEventListener('ended', () => {
			if (recorder.state !== 'inactive') { this.report(new Error(`Audio track ${trackId} ended unexpectedly`)); void this.stop().catch(() => {}); }
		}, { once: true });
		this.sequence.set(trackId, 0);
		this.started.set(trackId, this.origin);
		this.lastEventAt.set(trackId, performance.now());
		recorder.ondataavailable = (event) => {
			if (!event.data.size) return;
			if (this.pending.size >= 256 || this.pendingBytes + event.data.size > 16 * 1024 * 1024) {
				this.report(new Error('Browser audio conversion backlog exceeded; capture stopped'));
				for (const active of this.recorders.values()) if (active.state !== 'inactive') active.stop();
				this.streams.forEach((source) => source.getTracks().forEach((track) => track.stop()));
				return;
			}
			this.pendingBytes += event.data.size;
			const seq = this.sequence.get(trackId) ?? 0;
			this.sequence.set(trackId, seq + 1);
			const rate = 48_000;
			const now = performance.now();
			const previous = this.lastEventAt.get(trackId) ?? now;
			const start = Math.round((previous - (this.started.get(trackId) ?? previous)) * rate / 1000);
			const duration = Math.max(1, Math.round((now - previous) * rate / 1000));
			this.lastEventAt.set(trackId, now);
			const task = (this.conversion.get(trackId) ?? Promise.resolve()).then(async () => {
			const payload = new Uint8Array(await event.data.arrayBuffer());
			const block: AudioBlockEvent = {
				version: 1, session_id: sessionId, track_id: trackId, seq, sample_rate: rate,
				absolute_sample_start: start, duration_samples: duration,
				codec: mime.includes('ogg') ? 'opus' : 'opus', codec_config: mime,
				flags: { final: false, discontinuity: false }, idempotency_key: `${sessionId}:${trackId}:${seq}`,
				payload,
			};
			for (const listener of this.listeners) listener(block);
			});
			this.conversion.set(trackId, task);
			this.pending.add(task);
			void task.then(() => { this.pending.delete(task); this.pendingBytes -= event.data.size; }, (error) => { this.pending.delete(task); this.pendingBytes -= event.data.size; this.report(error instanceof Error ? error : new Error('Audio conversion failed')); });
		};
		recorder.start(250);
		this.recorders.set(trackId, recorder);
	}

	async pause(): Promise<void> { for (const recorder of this.recorders.values()) recorder.pause(); }
	async resume(): Promise<void> { for (const recorder of this.recorders.values()) recorder.resume(); }
	async stop(): Promise<void> {
		try {
		await Promise.all([...this.recorders.values()].map((recorder) => new Promise<void>((resolve) => {
			if (recorder.state === 'inactive') { resolve(); return; }
			recorder.addEventListener('stop', () => resolve(), { once: true });
			recorder.stop();
		})));
		await Promise.all([...this.pending]);
		} finally {
		this.streams.forEach((stream) => stream.getTracks().forEach((track) => track.stop()));
		this.recorders.clear();
		this.streams = [];
		this.meters.clear(); await this.meterContext?.close(); this.meterContext = undefined;
		}
		if (this.failure) throw this.failure;
	}
	onBlock(listener: (event: AudioBlockEvent) => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
}

export class OpfsArchiveWriter implements ArchiveWriter {
	private bytes = new Map<string, number>();
	private nextSequence = new Map<string, number>();
	async append(event: AudioBlockEvent): Promise<void> {
		assertAudioBlock(event);
		if (!event.payload) throw new Error('audio block payload is required');
		const mime = event.codec_config ?? 'audio/webm';
		const extension = mime.includes('ogg') ? 'ogg' : 'webm';
		const path = `sessions/${event.session_id}/${event.track_id}.${extension}`;
		const key = `${event.session_id}:${event.track_id}`;
		if (!this.nextSequence.has(key)) {
			const blocks = (await archivedBlocks(event.session_id)).filter((block) => block.event.track_id === event.track_id);
			const committed = blocks.reduce((end, block) => Math.max(end, block.offset + block.bytes), 0);
			try {
				const file = await readOpfsArchive(path);
				if (file.size !== committed) throw new Error('Archive has an uncommitted tail; preserve it for recovery before resuming');
			} catch (error) {
				if (!(error instanceof DOMException && error.name === 'NotFoundError' && committed === 0)) throw error;
			}
			this.nextSequence.set(key, blocks.length);
		}
		const next = this.nextSequence.get(key)!;
		if (event.seq < next) return;
		if (event.seq !== next) throw new Error('Archive block sequence gap');
		const bytes = await appendOpfsChunk(path, new Blob([event.payload.buffer.slice(event.payload.byteOffset, event.payload.byteOffset + event.payload.byteLength) as ArrayBuffer], { type: mime }));
		const { payload: _payload, ...metadata } = event;
		await commitArchivedBlock({ event: metadata, offset: bytes - event.payload.byteLength, bytes: event.payload.byteLength }, { session_id: event.session_id, track_id: event.track_id, path, mime_type: mime, bytes, block_count: event.seq + 1, created_ms: Date.now(), duration_samples: event.absolute_sample_start + event.duration_samples });
		this.nextSequence.set(key, next + 1);
		this.bytes.set(`${event.session_id}:${event.track_id}`, bytes);
	}
	async finalize(sessionId: string): Promise<{ content_hash: string; duration_samples: number }> {
		const archives = (await listBrowserMetadata()).filter((entry) => entry.session_id === sessionId && String(entry.key).startsWith('archive:'));
		const hashes: string[] = [];
		for (const archive of archives) {
			const file = await readOpfsArchive(String(archive.path));
			if (file.size !== Number(archive.bytes)) throw new Error('Local archive size mismatch');
			// WebCrypto digest requires a complete buffer. Large archives remain
			// explicitly unhashed rather than allocating unbounded recording memory.
			if (file.size > 16 * 1024 * 1024) continue;
			const digest = await crypto.subtle.digest('SHA-256', await file.arrayBuffer());
			const content_hash = [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
			await putArchiveMeta({ session_id: sessionId, track_id: archive.track_id as AudioBlockEvent['track_id'], path: String(archive.path), mime_type: String(archive.mime_type), bytes: file.size, block_count: Number(archive.block_count), created_ms: Number(archive.created_ms), content_hash, duration_samples: Number(archive.duration_samples ?? 0) });
			hashes.push(content_hash);
		}
		return { content_hash: hashes.length === 1 ? hashes[0] : '', duration_samples: Math.max(0, ...archives.map((archive) => Number(archive.duration_samples ?? 0))) };
	}
	async abort(_sessionId: string): Promise<void> {}
}

export class DummyRecognitionEngine implements RecognitionEngine {
	readonly kind = 'browser' as const;
	private listeners = new Set<(event: import('./contracts').TranscriptSegmentEvent) => void>();
	async start(_sessionId: string): Promise<void> {}
	async cancel(_sessionId: string): Promise<void> {}
	onSegment(listener: (event: import('./contracts').TranscriptSegmentEvent) => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
}

export class IndexedDbTranscriptRepository implements TranscriptRepository {
	async getSession(sessionId: string) { return getSessionState(sessionId); }
	async putSegment(event: import('./contracts').TranscriptSegmentEvent): Promise<void> { assertSegment(event); await putSegment(event); }
	async changesSince(cursor: string | null) { return changesSince(cursor?.split(':')[0] ?? '', Number(cursor?.split(':')[1] ?? -1)); }
}
