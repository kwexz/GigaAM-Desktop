import { describe, expect, it, vi, afterEach } from 'vitest';
import { WebSession } from './web-session';
import * as storage from './browser-storage';

describe('WebSession', () => {
	afterEach(() => vi.restoreAllMocks());
	it('stops capture even if storage fails when Stop begins', async () => {
		const persist = vi.spyOn(storage, 'putSessionState').mockResolvedValue();
		const session = new WebSession('quota-stop');
		vi.spyOn(session.capture, 'start').mockResolvedValue();
		const stop = vi.spyOn(session.capture, 'stop').mockResolvedValue();
		vi.spyOn(session.archive, 'finalize').mockResolvedValue({ content_hash: '', duration_samples: 0 });
		await session.start();
		persist.mockRejectedValue(new DOMException('Quota exceeded', 'QuotaExceededError'));
		await expect(session.stop()).rejects.toThrow('Quota exceeded');
		expect(stop).toHaveBeenCalledOnce();
		expect(session.status().recording).toBe('failed');
	});
	it('persists final capture blocks before finalizing the archive', async () => {
		vi.spyOn(storage, 'putSessionState').mockResolvedValue();
		const session = new WebSession('final-block');
		let listener: Parameters<typeof session.capture.onBlock>[0] | undefined;
		vi.spyOn(session.capture, 'onBlock').mockImplementation((callback) => { listener = callback; return () => {}; });
		vi.spyOn(session.capture, 'start').mockResolvedValue();
		const order: string[] = [];
		vi.spyOn(session.archive, 'append').mockImplementation(async () => { await Promise.resolve(); order.push('append'); });
		vi.spyOn(session.archive, 'finalize').mockImplementation(async () => { order.push('finalize'); return { content_hash: '', duration_samples: 0 }; });
		vi.spyOn(session.capture, 'stop').mockImplementation(async () => {
			listener?.({ version: 1, session_id: 'final-block', track_id: 'microphone', seq: 0, sample_rate: 48000, absolute_sample_start: 0, duration_samples: 480, codec: 'opus', flags: { final: true, discontinuity: false }, idempotency_key: 'final-block:microphone:0', payload: new Uint8Array([1]) });
		});
		await session.start();
		await session.stop();
		expect(order).toEqual(['append', 'finalize']);
		expect(session.status().recording).toBe('completed');
	});
	it('cleans up a partially acquired capture after permission failure', async () => {
		vi.spyOn(storage, 'putSessionState').mockResolvedValue();
		const session = new WebSession('permission-failure');
		vi.spyOn(session.capture, 'start').mockRejectedValue(new Error('Permission denied'));
		const stop = vi.spyOn(session.capture, 'stop').mockResolvedValue();
		await expect(session.start()).rejects.toThrow('Permission denied');
		expect(stop).toHaveBeenCalledOnce();
		expect(session.status().recording).toBe('failed');
	});
	it('starts in explicit local-first state and keeps remote optional', () => {
		const session = new WebSession('00000000-0000-4000-8000-000000000001');
		expect(session.status()).toEqual({ recording: 'idle', upload: 'not_started', recognition: 'not_started', sync: 'disabled' });
		expect(session.remote).toBeUndefined();
	});
});
