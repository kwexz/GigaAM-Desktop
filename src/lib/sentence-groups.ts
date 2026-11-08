/** Group consecutive recognition segments without changing their stored text or timing. */
export function groupSentenceSegments<T extends { text: string; source_track: string; speaker_id?: string }>(segments: T[]): T[][] {
	const groups: T[][] = [];
	for (const segment of segments) {
		const group = groups.at(-1);
		const previous = group?.at(-1);
		if (!group || !previous || previous.source_track !== segment.source_track || previous.speaker_id !== segment.speaker_id || /[.!?…][»”"')\]]*\s*$/u.test(previous.text)) {
			groups.push([segment]);
		} else {
			group.push(segment);
		}
	}
	return groups;
}
