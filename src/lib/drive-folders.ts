import type { OrganizationNode } from './organization';

/** Folder paths are presentation only; snapshots preserve distinct identities. */
export function driveFolders(branches: ReadonlyArray<{ file_id: string; organization?: OrganizationNode[] | null }>): Array<{ key: string; path: string }> {
	const result: Array<{ key: string; path: string }> = [];
	for (const branch of branches) {
		const nodes = branch.organization ?? [];
		for (const folder of nodes.filter((node) => node.kind === 'folder')) {
			const names = [folder.name];
			const seen = new Set([folder.id]);
			let parent = folder.parent_id;
			while (parent && !seen.has(parent)) {
				seen.add(parent);
				const ancestor = nodes.find((node) => node.id === parent && node.kind === 'folder');
				if (!ancestor) break;
				names.unshift(ancestor.name);
				parent = ancestor.parent_id;
			}
			result.push({ key: JSON.stringify([branch.file_id, folder.id]), path: names.join(' / ') });
		}
	}
	return result.sort((a, b) => a.path.localeCompare(b.path) || a.key.localeCompare(b.key));
}
