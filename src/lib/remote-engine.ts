import type { AudioBlockEvent, RecognitionEngine, TranscriptSegmentEvent } from './contracts';
import { assertAudioBlock, assertSegment } from './contracts';

export type RemoteEngineOptions = {
	url: string;
	maxQueuedBlocks?: number;
	reconnectMs?: number;
};

type WireBlock = Omit<AudioBlockEvent, 'payload'> & { checksum: string };

/**
 * Remote Live transport. Capture and archive remain independent: a full or
 * disconnected socket only pauses transport and keeps a bounded replay queue.
 * ACK advances the durable cursor per track; duplicate ACKs/segments are
 * idempotently ignored.
 */
export class RemoteEngine implements RecognitionEngine {
	readonly kind = 'remote' as const;
	private socket?: WebSocket;
	private sessionId = '';
	private readonly options: Required<RemoteEngineOptions>;
	private readonly queue: Array<{ event: AudioBlockEvent; wire: WireBlock }> = [];
	private readonly listeners = new Set<(event: TranscriptSegmentEvent) => void>();
	private readonly durable = new Map<AudioBlockEvent['track_id'], number>();
	private readonly sent = new Set<string>();
	private reconnectTimer?: number;
	private stopped = false;
	private connecting = false;

	constructor(options: RemoteEngineOptions) {
		this.options = { maxQueuedBlocks: 256, reconnectMs: 500, ...options };
	}

	async start(sessionId: string): Promise<void> {
		this.sessionId = sessionId;
		this.stopped = false;
		await this.connect();
	}

	async cancel(_sessionId: string): Promise<void> {
		this.stopped = true;
		if (this.reconnectTimer !== undefined) window.clearTimeout(this.reconnectTimer);
		this.socket?.close(1000, 'cancelled');
		this.socket = undefined;
		this.queue.length = 0;
	}

	async enqueue(event: AudioBlockEvent): Promise<void> {
		assertAudioBlock(event);
		if (event.session_id !== this.sessionId) throw new Error('remote session mismatch');
		const wire = { ...event, payload: undefined, checksum: await checksum(event.payload ?? new Uint8Array()) };
		if (this.queue.length >= this.options.maxQueuedBlocks) throw new Error('remote transport backlog is full');
		this.queue.push({ event, wire });
		this.flush();
	}

	durableSequence(track: AudioBlockEvent['track_id']): number { return this.durable.get(track) ?? -1; }
	backlog(): number { return this.queue.length; }

	onSegment(listener: (event: TranscriptSegmentEvent) => void): () => void {
	this.listeners.add(listener);
	return () => this.listeners.delete(listener);
	}

	private connect(): Promise<void> {
		if (this.connecting || this.socket?.readyState === WebSocket.OPEN) return Promise.resolve();
		this.connecting = true;
		return new Promise((resolve, reject) => {
			const socket = new WebSocket(`${this.options.url.replace(/\/$/, '')}/v1/sessions/${encodeURIComponent(this.sessionId)}/live`);
			this.socket = socket;
			socket.binaryType = 'arraybuffer';
			socket.onopen = () => { this.connecting = false; this.flush(); resolve(); };
			socket.onerror = () => { this.connecting = false; this.scheduleReconnect(); reject(new Error('remote websocket unavailable')); };
			socket.onclose = () => { this.connecting = false; if (!this.stopped) this.scheduleReconnect(); };
			socket.onmessage = (message) => this.handleMessage(message.data);
		});
	}

	private handleMessage(data: unknown): void {
		if (typeof data !== 'string') return;
		let value: { type?: string; track_id?: AudioBlockEvent['track_id']; seq?: number; durable?: boolean; segment?: TranscriptSegmentEvent };
		try { value = JSON.parse(data); } catch { return; }
		if (value.type === 'ack' && value.durable && value.track_id && value.seq !== undefined) {
			this.durable.set(value.track_id, Math.max(this.durableSequence(value.track_id), value.seq));
			const key = `${this.sessionId}:${value.track_id}:${value.seq}`;
			this.sent.delete(key);
			const index = this.queue.findIndex((item) => item.event.track_id === value.track_id && item.event.seq === value.seq);
			if (index >= 0) this.queue.splice(index, 1);
			this.flush();
		}
		if (value.segment) {
			try { assertSegment(value.segment); for (const listener of this.listeners) listener(value.segment); } catch { /* reject malformed remote result */ }
		}
	}

	private flush(): void {
		if (this.socket?.readyState !== WebSocket.OPEN) { this.scheduleReconnect(); return; }
		while (this.queue.length) {
			const item = this.queue[0];
			const key = item.event.idempotency_key;
			if (this.sent.has(key)) { this.queue.push(this.queue.shift()!); continue; }
			if (this.socket.bufferedAmount > 4 * 1024 * 1024) return;
			this.sent.add(key);
			this.socket.send(JSON.stringify(item.wire));
			if (item.event.payload) this.socket.send(item.event.payload.buffer.slice(item.event.payload.byteOffset, item.event.payload.byteOffset + item.event.payload.byteLength) as ArrayBuffer);
			break;
		}
	}

	private scheduleReconnect(): void {
		if (this.stopped || this.reconnectTimer !== undefined) return;
		this.reconnectTimer = window.setTimeout(() => {
			this.reconnectTimer = undefined;
			void this.connect().catch(() => this.scheduleReconnect());
		}, this.options.reconnectMs);
	}
}

async function checksum(payload: Uint8Array): Promise<string> {
	const digest = await crypto.subtle.digest('SHA-256', payload.buffer.slice(payload.byteOffset, payload.byteOffset + payload.byteLength) as ArrayBuffer);
	return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}
