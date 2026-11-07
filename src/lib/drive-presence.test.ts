import { describe, expect, it } from 'vitest';
import { drivePresence } from './drive-presence';

describe('Drive revision evidence', () => {
	const lineage = { original_id: 'recording-a', revision: 'rev-1' };
	it('does not infer status from incomplete discovery or missing lineage', () => {
		expect(drivePresence(lineage, [], false)).toBe('unknown');
		expect(drivePresence(undefined, [], true)).toBe('unknown');
	});
	it('does not treat synthetic legacy or empty identities as revision evidence', () => {
		expect(drivePresence({ original_id: 'recording-a', revision: 'legacy' }, [{ original_id: 'recording-a', revision: 'legacy', concurrent: false }], true)).toBe('unknown');
		expect(drivePresence({ original_id: '', revision: 'rev-1' }, [], true)).toBe('unknown');
		expect(drivePresence({ original_id: 'recording-a', revision: '' }, [], true)).toBe('unknown');
	});
	it('requires both original identity and revision', () => {
		expect(drivePresence(lineage, [{ original_id: 'other', revision: 'rev-1', concurrent: false }], true)).toBe('unknown');
		expect(drivePresence(lineage, [{ original_id: 'recording-a', revision: 'rev-1', concurrent: false }], true)).toBe('revision-found');
	});
	it('does not claim a revision is absent when only newer heads are exposed', () => {
		expect(drivePresence(lineage, [{ original_id: 'recording-a', revision: 'rev-2', concurrent: false }], true)).toBe('unknown');
	});
	it('reports conflicts even if one branch matches', () => {
		expect(drivePresence(lineage, [{ original_id: 'recording-a', revision: 'rev-1', concurrent: true }], true)).toBe('conflict');
	});
	it('detects conflict for a restored historical revision with two newer heads', () => {
		expect(drivePresence(lineage, [
			{ original_id: 'recording-a', revision: 'rev-1', concurrent: false },
			{ original_id: 'recording-a', revision: 'rev-2', concurrent: true },
			{ original_id: 'recording-a', revision: 'rev-3', concurrent: true }
		], true)).toBe('conflict');
	});
	it('identifies a historical revision without claiming it is current', () => {
		expect(drivePresence(lineage, [
			{ original_id: 'recording-a', revision: 'rev-1', concurrent: false },
			{ original_id: 'recording-a', revision: 'rev-2', concurrent: false }
		], true)).toBe('revision-found');
	});
});
