import type { AudioBlockEvent, SessionState } from './contracts';
import { BrowserCaptureSource, IndexedDbTranscriptRepository, OpfsArchiveWriter } from './browser-recorder';
import { putSessionState } from './browser-storage';
import { RemoteEngine } from './remote-engine';
import { LiveAsr } from './live-asr';

export type WebSessionOptions = {
	liveAsr?: boolean;
	remoteUrl?: string;
	token?: string;
};

/** Local-first web session coordinator. Capture/archive are never awaited on
 * network or inference; remote transport is a best-effort consumer. */
export class WebSession {
	readonly capture = new BrowserCaptureSource();
	readonly archive = new OpfsArchiveWriter();
	readonly repository = new IndexedDbTranscriptRepository();
	readonly remote?: RemoteEngine;
	readonly sessionId: string;
	private state: SessionState = {
		recording: 'idle', upload: 'not_started', recognition: 'not_started', sync: 'disabled',
	};
	private unsubscribe?: () => void;
	private pending: Promise<void> = Promise.resolve();
	private archiveError?: unknown;
	private queuedBytes = 0;
	private queuedBlocks = 0;
	private liveAsr?: LiveAsr;
	private releaseRecordingLock?: () => void;

	constructor(sessionId: string = crypto.randomUUID(), options: WebSessionOptions = {}) {
		this.sessionId = sessionId;
		if (options.liveAsr) {
			this.liveAsr = new LiveAsr();
			this.capture.onStream = (track, stream, offset) => this.liveAsr!.attach(track, stream, offset);
			this.liveAsr.onError = () => { this.state = { ...this.state, recognition: 'failed' }; void putSessionState(this.sessionId, this.state).catch(() => {}); };
			this.liveAsr.onResult = async (track, start, segments) => {
				for (const [index, segment] of segments.entries()) await this.repository.putSegment({ version: 1, session_id: this.sessionId, segment_id: `${track}:${start}:${index}`, track_id: track as 'microphone' | 'system', seq: index, start_seconds: start + segment.start_seconds, end_seconds: start + segment.end_seconds, text: segment.text, final: true, revision: 1, idempotency_key: `${this.sessionId}:${track}:${start}:${index}` });
			};
		}
		this.capture.onError((error) => {
			this.archiveError = error;
			this.state = { ...this.state, recording: 'failed' };
			void putSessionState(this.sessionId, this.state).catch(() => {});
		});
		if (options.remoteUrl) {
			this.remote = new RemoteEngine({ url: options.remoteUrl, token: options.token });
			this.remote.onSegment((segment) => {
				void this.repository.putSegment(segment).catch(() => {
					this.state = { ...this.state, recognition: 'failed' };
					void putSessionState(this.sessionId, this.state).catch(() => {});
				});
			});
		}
	}

	async start(source: 'microphone' | 'system' | 'system_and_microphone' = 'system_and_microphone'): Promise<void> {
		if (this.state.recording !== 'idle') throw new Error('Session has already started');
		if (navigator.locks) {
			await new Promise<void>((resolve, reject) => {
				void navigator.locks.request(`gigaam-recording:${this.sessionId}`, { ifAvailable: true }, async (lock) => {
					if (!lock) { reject(new Error('Recording is active in another tab')); return; }
					await new Promise<void>((release) => { this.releaseRecordingLock = release; resolve(); });
				}).catch(reject);
			});
		}
		this.state = { ...this.state, recording: 'recording', upload: this.remote ? 'queued' : 'not_started' };
		try { await putSessionState(this.sessionId, this.state); }
		catch (error) { this.releaseRecordingLock?.(); this.state = { ...this.state, recording: 'failed' }; throw error; }
		this.unsubscribe = this.capture.onBlock((event) => {
			const bytes = event.payload?.byteLength ?? 0;
			if (this.queuedBytes + bytes > 16 * 1024 * 1024 || this.queuedBlocks >= 256) {
				this.archiveError = new Error('Local archive backlog exceeded 16 MiB; capture stopped');
				this.state = { ...this.state, recording: 'failed' };
				void this.capture.stop().catch(() => {});
				return;
			}
			this.queuedBytes += bytes;
			this.queuedBlocks += 1;
			this.pending = this.pending.then(() => this.persistBlock(event)).catch((error) => { this.archiveError = error; }).finally(() => { this.queuedBytes -= bytes; this.queuedBlocks -= 1; });
		});
		try {
			await this.capture.start(this.sessionId, source);
		} catch (error) {
			try { await this.capture.stop(); await this.pending; }
			finally { this.unsubscribe?.(); this.releaseRecordingLock?.(); }
			this.state = { ...this.state, recording: 'failed' };
			await putSessionState(this.sessionId, this.state).catch(() => {});
			throw error;
		}
		if (this.remote) void this.remote.start(this.sessionId).catch(() => { this.state = { ...this.state, upload: 'failed' }; });
	}

	private async persistBlock(event: AudioBlockEvent): Promise<void> {
		try {
			await this.archive.append(event);
		} catch (error) {
			this.archiveError = error;
			this.state = { ...this.state, recording: 'failed' };
			void this.capture.stop().catch(() => {});
			throw error;
		}
		try {
			if (this.remote) void this.remote.enqueue(event).catch(() => {
				this.state = { ...this.state, upload: 'failed' };
				void putSessionState(this.sessionId, this.state).catch(() => {});
			});
		} catch {
			this.state = { ...this.state, upload: 'failed' };
			await putSessionState(this.sessionId, this.state);
		}
	}

	async pause(): Promise<void> { await this.capture.pause(); this.liveAsr?.pause(); this.state = { ...this.state, recording: 'paused' }; await putSessionState(this.sessionId, this.state); }
	async resume(): Promise<void> { await this.capture.resume(); this.liveAsr?.resume(); this.state = { ...this.state, recording: 'recording' }; await putSessionState(this.sessionId, this.state); }
	async stop(): Promise<void> {
		try {
		this.state = { ...this.state, recording: 'stopping' };
		await putSessionState(this.sessionId, this.state).catch(() => {});
		await this.capture.stop();
		await this.pending;
		this.unsubscribe?.();
		if (this.archiveError) throw this.archiveError;
		await this.archive.finalize(this.sessionId);
		if (this.liveAsr) void this.liveAsr.stop().catch(() => { this.state = { ...this.state, recognition: 'failed' }; });
		this.state = { ...this.state, recording: 'completed' };
		await putSessionState(this.sessionId, this.state);
		} catch (error) {
			this.state = { ...this.state, recording: 'failed' };
			await putSessionState(this.sessionId, this.state).catch(() => {});
			throw error;
		} finally { this.unsubscribe?.(); this.releaseRecordingLock?.(); this.releaseRecordingLock = undefined; }
	}

	status(): SessionState { return this.state; }
}
