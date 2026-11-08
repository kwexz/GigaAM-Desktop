import { describe, expect, it } from 'vitest';
import { deleteFolder, moveNode, renameFolder, validateOrganization, type OrganizationNode } from './organization';

const fixture = (): OrganizationNode[] => [
	{ id: 'folder', kind: 'folder', parent_id: null, name: 'Folder', position: 0 },
	{ id: 'nested', kind: 'folder', parent_id: 'folder', name: 'Nested', position: 0 },
	{ id: 'a', kind: 'recording', parent_id: null, name: 'A', position: 1 },
	{ id: 'b', kind: 'recording', parent_id: null, name: 'B', position: 2 }
];
describe('organization transaction planner', () => {
	it('validates duplicate IDs, parent references and positions before persistence', () => {
		expect(() => validateOrganization(fixture())).not.toThrow();
		expect(() => validateOrganization([...fixture(), fixture()[0]])).toThrow('Invalid');
		expect(() => validateOrganization([{ ...fixture()[0], position: -1 }])).toThrow('Invalid');
		expect(() => validateOrganization([{ ...fixture()[0], parent_id: 'missing' }])).toThrow('parent');
	});
	it('validates folder rename without changing identity or parent', () => {
		expect(renameFolder(fixture(), 'folder', ' Renamed ')[0]).toMatchObject({ id: 'folder', name: 'Renamed', parent_id: null });
		expect(() => renameFolder(fixture(), 'folder', ' ')).toThrow('name');
	});
	it('moves records without mutating input or identity', () => {
		const nodes = fixture();
		const moved = moveNode(nodes, 'a', 'nested', 0);
		expect(moved.find((node) => node.id === 'a')).toMatchObject({ id: 'a', parent_id: 'nested', position: 0 });
		expect(nodes.find((node) => node.id === 'a')?.parent_id).toBeNull();
	});
	it('rejects missing parent and cyclic nesting', () => {
		expect(() => moveNode(fixture(), 'a', null, NaN)).toThrow('position');
		expect(() => moveNode(fixture(), 'folder', 'nested', 0)).toThrow('cycle');
		expect(() => moveNode(fixture(), 'a', 'missing', 0)).toThrow('parent');
	});
	it('reorders siblings without touching another parent', () => {
		const result = moveNode(fixture(), 'b', null, 1);
		expect(result.find((node) => node.id === 'b')?.position).toBe(1);
		expect(result.find((node) => node.id === 'a')?.position).toBe(2);
		expect(result.find((node) => node.id === 'nested')?.position).toBe(0);
	});
	it('deletes a folder without deleting children', () => {
		const result = deleteFolder(moveNode(fixture(), 'a', 'folder', 1), 'folder');
		expect(result.map((node) => node.id).sort()).toEqual(['a', 'b', 'nested']);
		expect(result.every((node) => node.parent_id === null)).toBe(true);
	});
});
