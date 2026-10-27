import { describe, expect, it } from 'vitest';
import { assertAudioBlock, assertSegment, CONTRACT_VERSION } from './contracts';
import { createUnavailablePorts } from './ports';

describe('versioned runtime contracts', () => {
	it('accepts replayable audio identity and rejects invalid timeline', () => {
		const event = {
			version: CONTRACT_VERSION, session_id: 'session', track_id: 'microphone' as const,
			seq: 4, sample_rate: 48000, absolute_sample_start: 192000,
			duration_samples: 480, codec: 'pcm_f32le' as const,
			flags: { final: false, discontinuity: false }, idempotency_key: 'session:microphone:4',
		};
		assertAudioBlock(event);
		expect(() => assertAudioBlock({ ...event, seq: -1 })).toThrow('sequence');
	});

	it('accepts revisions and rejects stale-shaped timestamps', () => {
		const event = {
			version: CONTRACT_VERSION, session_id: 'session', segment_id: 'segment', track_id: 'system' as const,
			seq: 1, start_seconds: 1, end_seconds: 2, text: 'ok', final: true,
			revision: 2, idempotency_key: 'session:segment:2',
		};
		assertSegment(event);
		expect(() => assertSegment({ ...event, end_seconds: 0 })).toThrow('timestamps');
	});

	it('keeps offline desktop optionality explicit in the default adapter', async () => {
		const ports = createUnavailablePorts();
		expect(ports.capabilities.offline_queue).toBe(false);
		expect(ports.capabilities.cloud_sync).toBe('disabled');
		await expect(ports.capture.start('session')).rejects.toThrow('runtime adapter');
	});
});
