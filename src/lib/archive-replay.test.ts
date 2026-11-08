import { afterEach, describe, expect, it, vi } from 'vitest';
import { replayToRemote } from './archive-replay';
import * as storage from './browser-storage';
import { RemoteEngine } from './remote-engine';

describe('durable replay transport', () => {
	afterEach(() => vi.restoreAllMocks());
	it('reconstructs persisted payload and waits for bounded remote capacity', async () => {
		const event = { version: 1 as const, session_id: 'session', track_id: 'microphone' as const, seq: 0, sample_rate: 48000, absolute_sample_start: 0, duration_samples: 480, codec: 'opus' as const, flags: { final: false, discontinuity: false }, idempotency_key: 'session:microphone:0' };
		vi.spyOn(storage, 'archivedBlocks').mockResolvedValue([{ event, offset: 1, bytes: 2 }]);
		vi.spyOn(storage, 'listBrowserMetadata').mockResolvedValue([{ key: 'archive:session:microphone', session_id: 'session', track_id: 'microphone', path: 'audio.webm' }]);
		vi.spyOn(storage, 'readOpfsArchive').mockResolvedValue(new File([new Uint8Array([0, 1, 2])], 'audio.webm'));
		const engine = new RemoteEngine({ url: 'ws://localhost' });
		const capacity = vi.spyOn(engine, 'waitForCapacity').mockResolvedValue();
		const drain = vi.spyOn(engine, 'waitForDrain').mockResolvedValue();
		const enqueue = vi.spyOn(engine, 'enqueue').mockResolvedValue();
		await replayToRemote('session', engine);
		expect(capacity).toHaveBeenCalledOnce();
		expect(drain).toHaveBeenCalledOnce();
		expect(enqueue.mock.calls[0][0].payload).toEqual(new Uint8Array([1, 2]));
	});
});
