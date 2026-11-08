export type OrganizationNode = {
	id: string;
	kind: 'folder' | 'recording';
	parent_id: string | null;
	name: string;
	position: number;
};

export const ORGANIZATION_VERSION = 1 as const;

export const organizationLabels = {
	ru: { rename: 'Переименовать папку', delete: 'Удалить папку', folderUp: 'Папку выше', folderMove: 'Переместить папку', recordingUp: 'Запись выше', recordingDown: 'Запись ниже', recordingMove: 'Переместить запись' },
	en: { rename: 'Rename folder', delete: 'Delete folder', folderUp: 'Move folder up', folderMove: 'Move folder', recordingUp: 'Move recording up', recordingDown: 'Move recording down', recordingMove: 'Move recording' }
};

export function validateOrganization(nodes: OrganizationNode[]): void {
	if (nodes.length > 100_000) throw new Error('Organization limit exceeded');
	const ids = new Set<string>();
	for (const node of nodes) {
		if (!node.id || ids.has(node.id) || !Number.isSafeInteger(node.position) || node.position < 0 || !['folder', 'recording'].includes(node.kind)) throw new Error('Invalid organization node');
		if (node.kind === 'folder' && (!node.name.trim() || node.name.length > 256)) throw new Error('Invalid folder name');
		ids.add(node.id);
	}
	for (const node of nodes) {
		let parent = node.parent_id;
		const seen = new Set([node.id]);
		while (parent !== null) {
			if (seen.has(parent)) throw new Error('Folder cycle');
			seen.add(parent);
			const folder = nodes.find((entry) => entry.id === parent && entry.kind === 'folder');
			if (!folder) throw new Error('Unknown parent folder');
			parent = folder.parent_id;
		}
	}
}

/** Pure transaction planner shared by platform repositories. Audio paths and
 * recognition queue positions are deliberately absent from this model. */
export function moveNode(nodes: OrganizationNode[], id: string, parent: string | null, index: number): OrganizationNode[] {
	if (!Number.isFinite(index)) throw new Error('Invalid position');
	const result = nodes.map((node) => ({ ...node }));
	const target = result.find((node) => node.id === id);
	if (!target) throw new Error('Unknown item');
	if (parent !== null && !result.some((node) => node.id === parent && node.kind === 'folder')) throw new Error('Unknown parent folder');
	const visited = new Set<string>([id]);
	let ancestor = parent;
	while (ancestor !== null) {
		if (visited.has(ancestor)) throw new Error('Folder cycle');
		visited.add(ancestor);
		ancestor = result.find((node) => node.id === ancestor)?.parent_id ?? null;
	}
	const previous = target.parent_id;
	target.parent_id = parent;
	const siblings = result.filter((node) => node.id !== id && node.parent_id === parent).sort(compareNodes);
	siblings.splice(Math.max(0, Math.min(siblings.length, Math.trunc(index))), 0, target);
	siblings.forEach((node, position) => { node.position = position; });
	if (previous !== parent) result.filter((node) => node.parent_id === previous).sort(compareNodes).forEach((node, position) => { node.position = position; });
	return result;
}

export function deleteFolder(nodes: OrganizationNode[], id: string): OrganizationNode[] {
	const folder = nodes.find((node) => node.id === id && node.kind === 'folder');
	if (!folder) throw new Error('Unknown folder');
	let result = nodes.map((node) => ({ ...node }));
	for (const child of nodes.filter((node) => node.parent_id === id).sort(compareNodes)) {
		result = moveNode(result, child.id, folder.parent_id, result.length);
	}
	return result.filter((node) => node.id !== id);
}

export function compareNodes(a: OrganizationNode, b: OrganizationNode): number {
	return a.position - b.position || a.id.localeCompare(b.id);
}

export function renameFolder(nodes: OrganizationNode[], id: string, name: string): OrganizationNode[] {
	name = name.trim();
	if (!name || name.length > 256) throw new Error('Invalid folder name');
	if (!nodes.some((node) => node.id === id && node.kind === 'folder')) throw new Error('Unknown folder');
	return nodes.map((node) => node.id === id ? { ...node, name } : { ...node });
}
