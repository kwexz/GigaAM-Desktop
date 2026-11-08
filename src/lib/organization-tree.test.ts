import { expect, it } from 'vitest';
import { visibleTree } from './organization-tree';

it('collapses descendants without removing persisted nodes', () => {
	const nodes = [
		{ id: 'folder', kind: 'folder' as const, parent_id: null, name: 'Folder', position: 0 },
		{ id: 'recording', kind: 'recording' as const, parent_id: 'folder', name: 'Audio', position: 0 }
	];
	expect(visibleTree(nodes, new Set()).map((node) => [node.id, node.depth])).toEqual([['folder', 0], ['recording', 1]]);
	expect(visibleTree(nodes, new Set(['folder'])).map((node) => node.id)).toEqual(['folder']);
	expect(nodes).toHaveLength(2);
});
