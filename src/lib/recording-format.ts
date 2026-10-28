export type RecordingFormat = 'flac' | 'wav' | 'mp3' | 'ogg';
export const DEFAULT_RECORDING_FORMAT: RecordingFormat = 'flac';

export function savedRecordingFormat(storage: Pick<Storage, 'getItem'>): RecordingFormat | undefined {
	try {
		const value = storage.getItem('gigaam.archive-format');
		return value === 'flac' || value === 'wav' || value === 'mp3' || value === 'ogg' ? value : undefined;
	} catch { return undefined; }
}

export function saveRecordingFormat(storage: Pick<Storage, 'setItem'>, format: RecordingFormat): void {
	try { storage.setItem('gigaam.archive-format', format); } catch { /* Storage can be disabled; capture still works. */ }
}

export function browserRecordingMime(supported: (mime: string) => boolean): string {
	const mime = ['audio/ogg;codecs=opus', 'audio/webm;codecs=opus', 'audio/webm'].find(supported);
	if (!mime) throw new Error('No supported Opus/WebM or Ogg recorder');
	return mime;
}
