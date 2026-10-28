import { describe, expect, it } from 'vitest';
import { browserRecordingMime, savedRecordingFormat, saveRecordingFormat } from './recording-format';

describe('recording format preferences', () => {
	it('does not fail capture when preference storage is unavailable', () => {
		expect(() => saveRecordingFormat({ setItem: () => { throw new Error('disabled'); } }, 'ogg')).not.toThrow();
		expect(savedRecordingFormat({ getItem: () => { throw new Error('disabled'); } })).toBeUndefined();
	});
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
