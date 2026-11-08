import { describe, expect, it } from 'vitest';
import { groupSentenceSegments } from './sentence-groups';

describe('groupSentenceSegments', () => {
	it('joins a phrase cut across recognition segments and starts a group after punctuation', () => {
		const segments = [
			{ text: 'Конец фразы', source_track: 'file' },
			{ text: 'на следующей строке.', source_track: 'file' },
			{ text: 'Новое предложение', source_track: 'file' }
		];
		expect(groupSentenceSegments(segments)).toEqual([[segments[0], segments[1]], [segments[2]]]);
	});

	it('never joins different tracks or speakers', () => {
		const segments = [
			{ text: 'Один', source_track: 'microphone', speaker_id: 'a' },
			{ text: 'Два', source_track: 'system', speaker_id: 'a' },
			{ text: 'Три', source_track: 'system', speaker_id: 'b' }
		];
		expect(groupSentenceSegments(segments)).toEqual(segments.map((segment) => [segment]));
	});
});
