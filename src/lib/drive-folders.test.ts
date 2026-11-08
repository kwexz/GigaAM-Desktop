import { describe, expect, it } from 'vitest';
import { driveFolders } from './drive-folders';
import type { OrganizationNode } from './organization';

const folder = (id: string, name: string, parent_id: string | null = null): OrganizationNode => ({ id, name, parent_id, kind: 'folder', position: 0 });

describe('Drive snapshot folders', () => {
	it('preserves empty folders and distinct identities with identical names', () => {
		const folders = driveFolders([{ file_id: 'snapshot', organization: [folder('a', 'Same'), folder('b', 'Same')] }]);
		expect(folders.map((item) => item.path)).toEqual(['Same', 'Same']);
		expect(new Set(folders.map((item) => item.key)).size).toBe(2);
	});
	it('keeps snapshots separate and constructs nested paths', () => {
		const nodes = [folder('a', 'Parent'), folder('b', 'Empty', 'a')];
		const folders = driveFolders([{ file_id: 'one', organization: nodes }, { file_id: 'two', organization: nodes }]);
		expect(folders.filter((item) => item.path === 'Parent / Empty')).toHaveLength(2);
		expect(new Set(folders.map((item) => item.key)).size).toBe(4);
	});
});
