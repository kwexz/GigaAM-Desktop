import type { ArchiveWriter, AudioBlockEvent, CaptureSource, RecognitionEngine, TranscriptRepository } from './contracts';
import { assertAudioBlock, assertSegment } from './contracts';
import { appendOpfsChunk, changesSince, getSessionState, putArchiveMeta, putSegment, putSessionState, requestPersistentStorage } from './browser-storage';

export type BrowserCaptureStatus = { microphone: boolean; system: boolean; systemAudioUnavailable: boolean };

export class BrowserCaptureSource implements CaptureSource {
	private listeners = new Set<(event: AudioBlockEvent) => void>();
	private recorders = new Map<AudioBlockEvent['track_id'], MediaRecorder>();
	private sequence = new Map<AudioBlockEvent['track_id'], number>();
	private started = new Map<AudioBlockEvent['track_id'], number>();
	private lastEventAt = new Map<AudioBlockEvent['track_id'], number>();
	private streams: MediaStream[] = [];
	status: BrowserCaptureStatus = { microphone: false, system: false, systemAudioUnavailable: false };

	async start(sessionId: string): Promise<void> {
		await requestPersistentStorage();
		const mic = await navigator.mediaDevices.getUserMedia({ audio: true });
		this.streams.push(mic);
		this.startTrack(sessionId, 'microphone', mic);
		this.status.microphone = true;
		try {
			const display = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: true });
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
	}

	private startTrack(sessionId: string, trackId: AudioBlockEvent['track_id'], stream: MediaStream): void {
		const mime = ['audio/webm;codecs=opus', 'audio/ogg;codecs=opus', 'audio/webm'].find((candidate) => MediaRecorder.isTypeSupported(candidate));
		if (!mime) throw new Error('No supported Opus/WebM or Ogg recorder');
		const recorder = new MediaRecorder(stream, { mimeType: mime });
		this.sequence.set(trackId, 0);
		this.started.set(trackId, performance.now());
		this.lastEventAt.set(trackId, performance.now());
		recorder.ondataavailable = async (event) => {
			if (!event.data.size) return;
			const seq = this.sequence.get(trackId) ?? 0;
			const rate = 48_000;
			const now = performance.now();
			const previous = this.lastEventAt.get(trackId) ?? now;
			const start = Math.round((previous - (this.started.get(trackId) ?? previous)) * rate / 1000);
			const duration = Math.max(1, Math.round((now - previous) * rate / 1000));
			this.lastEventAt.set(trackId, now);
			const payload = new Uint8Array(await event.data.arrayBuffer());
			const block: AudioBlockEvent = {
				version: 1, session_id: sessionId, track_id: trackId, seq, sample_rate: rate,
				absolute_sample_start: start, duration_samples: duration,
				codec: mime.includes('ogg') ? 'opus' : 'opus', codec_config: mime,
				flags: { final: false, discontinuity: false }, idempotency_key: `${sessionId}:${trackId}:${seq}`,
				payload,
			};
			this.sequence.set(trackId, seq + 1);
			for (const listener of this.listeners) listener(block);
		};
		recorder.start(250);
		this.recorders.set(trackId, recorder);
	}

	async pause(): Promise<void> { for (const recorder of this.recorders.values()) recorder.pause(); }
	async resume(): Promise<void> { for (const recorder of this.recorders.values()) recorder.resume(); }
	async stop(): Promise<void> {
		for (const recorder of this.recorders.values()) if (recorder.state !== 'inactive') recorder.stop();
		this.streams.forEach((stream) => stream.getTracks().forEach((track) => track.stop()));
		this.recorders.clear();
		this.streams = [];
	}
	onBlock(listener: (event: AudioBlockEvent) => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
}

export class OpfsArchiveWriter implements ArchiveWriter {
	private bytes = new Map<string, number>();
	async append(event: AudioBlockEvent): Promise<void> {
		assertAudioBlock(event);
		if (!event.payload) throw new Error('audio block payload is required');
		const path = `sessions/${event.session_id}/${event.track_id}.webm`;
		const bytes = await appendOpfsChunk(path, new Blob([event.payload.buffer.slice(event.payload.byteOffset, event.payload.byteOffset + event.payload.byteLength) as ArrayBuffer], { type: 'audio/webm' }));
		this.bytes.set(`${event.session_id}:${event.track_id}`, bytes);
		await putArchiveMeta({ session_id: event.session_id, track_id: event.track_id, path, mime_type: 'audio/webm', bytes, block_count: event.seq + 1, created_ms: Date.now() });
	}
	async finalize(sessionId: string): Promise<{ content_hash: string; duration_samples: number }> {
		return { content_hash: `${sessionId}:opfs`, duration_samples: 0 };
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
