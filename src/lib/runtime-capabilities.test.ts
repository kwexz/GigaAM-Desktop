import { expect, it } from 'vitest';
import { runtimeCapabilities } from './runtime-capabilities';

it('does not promise universal browser system audio or connected cloud', () => {
	expect(runtimeCapabilities(false)).toMatchObject({ system_audio: 'optional', playback: true, offline_queue: true, cloud_sync: 'disabled' });
	expect(runtimeCapabilities(true).system_audio).toBe('available');
});
