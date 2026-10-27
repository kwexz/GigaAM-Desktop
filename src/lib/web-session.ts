import type { AudioBlockEvent, SessionState } from './contracts';
import { BrowserCaptureSource, IndexedDbTranscriptRepository, OpfsArchiveWriter } from './browser-recorder';
import { putSessionState } from './browser-storage';
import { RemoteEngine } from './remote-engine';

export type WebSessionOptions = {
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

	constructor(sessionId = crypto.randomUUID(), options: WebSessionOptions = {}) {
		this.sessionId = sessionId;
		if (options.remoteUrl) {
			this.remote = new RemoteEngine({ url: options.remoteUrl, token: options.token });
		}
	}

	async start(): Promise<void> {
		this.state = { ...this.state, recording: 'recording', upload: this.remote ? 'queued' : 'not_started' };
		await putSessionState(this.sessionId, this.state);
		this.unsubscribe = this.capture.onBlock((event) => { void this.persistBlock(event); });
		await this.capture.start(this.sessionId);
		if (this.remote) void this.remote.start(this.sessionId).catch(() => { this.state = { ...this.state, upload: 'failed' }; });
	}

	private async persistBlock(event: AudioBlockEvent): Promise<void> {
		try {
			await this.archive.append(event);
			if (this.remote) await this.remote.enqueue(event);
		} catch {
			this.state = { ...this.state, upload: 'failed' };
			await putSessionState(this.sessionId, this.state);
		}
	}

	async pause(): Promise<void> { await this.capture.pause(); this.state = { ...this.state, recording: 'paused' }; await putSessionState(this.sessionId, this.state); }
	async resume(): Promise<void> { await this.capture.resume(); this.state = { ...this.state, recording: 'recording' }; await putSessionState(this.sessionId, this.state); }
	async stop(): Promise<void> {
		this.state = { ...this.state, recording: 'stopping' };
		await putSessionState(this.sessionId, this.state);
		await this.capture.stop();
		this.unsubscribe?.();
		await this.archive.finalize(this.sessionId);
		this.state = { ...this.state, recording: 'completed' };
		await putSessionState(this.sessionId, this.state);
	}

	status(): SessionState { return this.state; }
}
