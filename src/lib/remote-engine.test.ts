import { describe, expect, it } from 'vitest';
import { RemoteEngine } from './remote-engine';

describe('RemoteEngine', () => {
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
