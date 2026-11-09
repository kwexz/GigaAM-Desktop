/** Sentence fragments retain their source segment; no word-level timestamps are invented. */
export function groupSentenceSegments<T extends { text: string; source_track: string; speaker_id?: string | null; start_seconds?: number; end_seconds?: number }>(segments: T[]): { segment: T; text: string; key: string }[][] {
	const groups: { segment: T; text: string; key: string }[][] = [];
	for (const [segmentIndex, segment] of segments.entries()) {
		const parts = [segment.text];
		for (const [index, part] of parts.entries()) {
			const text = part.trim();
			if (!text) continue;
			const group = groups.at(-1);
			const previous = group?.at(-1);
			const item = { segment, text, key: `${segmentIndex}:${index}` };
			const gap = previous && segment.start_seconds !== undefined && previous.segment.end_seconds !== undefined ? segment.start_seconds - previous.segment.end_seconds : 0;
			const length = group?.reduce((sum, fragment) => sum + fragment.text.length + 1, 0) ?? 0;
			if (!group || !previous || previous.segment.source_track !== segment.source_track || previous.segment.speaker_id !== segment.speaker_id || gap > 2 || (length >= 240 && /[.!?…][»”"')\]]*\s*$/u.test(previous.text))) groups.push([item]);
			else group.push(item);
		}
	}
	return groups;
}
