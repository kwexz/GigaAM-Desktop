import { describe, expect, it } from 'vitest';
import { RemoteEngine } from './remote-engine';

describe('web resilience contracts', () => {
	it('bounds backlog and exposes lag instead of blocking capture', async () => {
		const engine = new RemoteEngine({ url: 'ws://127.0.0.1:1', maxQueuedBlocks: 2 });
		await expect(engine.start('offline')).rejects.toThrow();
		const make = (seq: number) => ({ version: 1 as const, session_id: 'offline', track_id: 'system' as const, seq, sample_rate: 48000, absolute_sample_start: seq * 480, duration_samples: 480, codec: 'opus' as const, flags: { final: false, discontinuity: false }, idempotency_key: `offline:system:${seq}`, payload: new Uint8Array([seq]) });
		await engine.enqueue(make(0)); await engine.enqueue(make(1));
		expect(engine.backlog()).toBe(2);
		await expect(engine.enqueue(make(2))).rejects.toThrow('backlog');
	});
});
