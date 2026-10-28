import { describe, expect, it, vi } from 'vitest';
import { RemoteEngine } from './remote-engine';

describe('RemoteEngine', () => {
	it('accepts durable ACKs idempotently without removing another sequence', async () => {
		let socket: Socket;
		class Socket {
			static OPEN = 1;
			readyState = 1;
			send = vi.fn();
			onopen?: () => void;
			onmessage?: (message: { data: string }) => void;
			constructor() { socket = this; queueMicrotask(() => this.onopen?.()); }
		}
		vi.stubGlobal('WebSocket', Socket);
		try {
			const engine = new RemoteEngine({ url: 'ws://localhost' });
			await engine.start('session');
			await engine.enqueue({ version: 1, session_id: 'session', track_id: 'microphone', seq: 0, sample_rate: 48000, absolute_sample_start: 0, duration_samples: 480, codec: 'opus', flags: { final: false, discontinuity: false }, idempotency_key: 'session:microphone:0', payload: new Uint8Array([1]) });
			socket!.onmessage?.({ data: JSON.stringify({ type: 'ack', durable: true, track_id: 'microphone', seq: 999 }) });
			expect(engine.durableSequence('microphone')).toBe(-1);
			expect(engine.backlog()).toBe(1);
			const ack = JSON.stringify({ type: 'ack', durable: true, track_id: 'microphone', seq: 0 });
			socket!.onmessage?.({ data: ack });
			socket!.onmessage?.({ data: ack });
			expect(engine.backlog()).toBe(0);
			expect(engine.durableSequence('microphone')).toBe(0);
		} finally { vi.unstubAllGlobals(); }
	});
	it('does not spin when flushing an already sent block awaiting ACK', async () => {
		class Socket {
			static OPEN = 1;
			readyState = 1;
			send = vi.fn();
			onopen?: () => void;
			constructor() { queueMicrotask(() => this.onopen?.()); }
		}
		vi.stubGlobal('WebSocket', Socket);
		try {
			const engine = new RemoteEngine({ url: 'ws://localhost' });
			await engine.start('session');
			for (const seq of [0, 1]) await engine.enqueue({
				version: 1, session_id: 'session', track_id: 'microphone', seq,
				sample_rate: 48000, absolute_sample_start: seq * 480, duration_samples: 480,
				codec: 'opus', flags: { final: false, discontinuity: false },
				idempotency_key: `session:microphone:${seq}`, payload: new Uint8Array([1])
			});
			expect(engine.backlog()).toBe(2);
		} finally { vi.unstubAllGlobals(); }
	});
	it('rejects a full bounded transport queue before network send', async () => {
		const engine = new RemoteEngine({ url: 'ws://127.0.0.1:1', maxQueuedBlocks: 1 });
		await expect(engine.start('session')).rejects.toThrow();
		const block = (seq: number) => ({
			version: 1 as const, session_id: 'session', track_id: 'microphone' as const, seq,
			sample_rate: 48000, absolute_sample_start: seq * 480, duration_samples: 480,
			codec: 'opus' as const, flags: { final: false, discontinuity: false },
			idempotency_key: `session:microphone:${seq}`, payload: new Uint8Array([1]),
		});
		await engine.enqueue(block(0));
		await expect(engine.enqueue(block(1))).rejects.toThrow('backlog');
	});
});
