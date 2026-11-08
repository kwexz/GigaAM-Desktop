/** Sentence fragments retain their source segment; no word-level timestamps are invented. */
export function groupSentenceSegments<T extends { text: string; source_track: string; speaker_id?: string }>(segments: T[]): { segment: T; text: string; key: string }[][] {
	const groups: { segment: T; text: string; key: string }[][] = [];
	for (const [segmentIndex, segment] of segments.entries()) {
		const parts = segment.text.match(/.*?[.!?…]+[»”"')\]]*(?=\s+[«“"(]?[A-ZА-ЯЁ0-9]|$)|.+$/gu) ?? [segment.text];
		for (const [index, part] of parts.entries()) {
			const text = part.trim();
			if (!text) continue;
			const group = groups.at(-1);
			const previous = group?.at(-1);
			const item = { segment, text, key: `${segmentIndex}:${index}` };
			if (!group || !previous || previous.segment.source_track !== segment.source_track || previous.segment.speaker_id !== segment.speaker_id || /[.!?…][»”"')\]]*\s*$/u.test(previous.text)) groups.push([item]);
			else group.push(item);
		}
	}
	return groups;
}
