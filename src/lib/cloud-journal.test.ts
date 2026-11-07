import { describe, expect, it } from 'vitest';
import { mergeJournal } from './cloud-journal';
import type { TranscriptChange } from './contracts';

describe('cloud journal conflict preservation', () => {
	const change = (text: string): TranscriptChange => ({ change_id: 'change', session_id: 'session', kind: 'segment_upsert', revision: 1, payload: { text } });
	it('replays identical changes without duplication', () => {
		const result = mergeJournal([change('A')], [change('A')]);
		expect(result.changes).toHaveLength(1);
		expect(result.conflicts).toHaveLength(0);
	});
	it('retains conflicting text instead of silently overwriting', () => {
		const result = mergeJournal([change('A')], [change('B')]);
		expect(result.changes[0].payload).toEqual({ text: 'A' });
		expect(result.conflicts[0].payload).toEqual({ text: 'B' });
	});
});
