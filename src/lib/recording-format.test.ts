import { describe, expect, it } from 'vitest';
import { browserRecordingMime, savedRecordingFormat } from './recording-format';

describe('recording format preferences', () => {
	it('preserves explicit supported choices and rejects stale values', () => {
		expect(savedRecordingFormat({ getItem: () => 'flac' })).toBe('flac');
		expect(savedRecordingFormat({ getItem: () => 'ogg' })).toBe('ogg');
		expect(savedRecordingFormat({ getItem: () => 'unknown' })).toBeUndefined();
	});
	it('negotiates actual Ogg support with an honest WebM fallback', () => {
		expect(browserRecordingMime(() => true)).toBe('audio/ogg;codecs=opus');
		expect(browserRecordingMime((mime) => mime.startsWith('audio/webm'))).toBe('audio/webm;codecs=opus');
		expect(() => browserRecordingMime(() => false)).toThrow('supported');
	});
});
