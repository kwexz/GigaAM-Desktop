import { describe, expect, it } from 'vitest';
import { groupSentenceSegments } from './sentence-groups';

describe('groupSentenceSegments', () => {
	it('joins a phrase cut across recognition segments and starts a group after punctuation', () => {
		const segments = [
			{ text: 'Конец фразы', source_track: 'file' },
			{ text: 'на следующей строке.', source_track: 'file' },
			{ text: 'Новое предложение', source_track: 'file' }
		];
		expect(groupSentenceSegments(segments).map((group) => group.map((item) => item.segment))).toEqual([[segments[0], segments[1]], [segments[2]]]);
	});

	it('never joins different tracks or speakers', () => {
		const segments = [
			{ text: 'Один', source_track: 'microphone', speaker_id: 'a' },
			{ text: 'Два', source_track: 'system', speaker_id: 'a' },
			{ text: 'Три', source_track: 'system', speaker_id: 'b' }
		];
		expect(groupSentenceSegments(segments).map((group) => group.map((item) => item.segment))).toEqual(segments.map((segment) => [segment]));
	});

	it('splits multiple sentences while preserving the original editable segment', () => {
		const segment = { text: 'Первое предложение. Второе предложение!', source_track: 'file' };
		const result = groupSentenceSegments([segment]);
		expect(result.map((group) => group[0].text)).toEqual(['Первое предложение.', 'Второе предложение!']);
		expect(result.every((group) => group[0].segment === segment)).toBe(true);
	});
});
