import type { OrganizationNode } from './organization';
import { compareNodes } from './organization';

export function visibleTree(nodes: OrganizationNode[], collapsed: ReadonlySet<string>): Array<OrganizationNode & { depth: number }> {
	const result: Array<OrganizationNode & { depth: number }> = [];
	const visited = new Set<string>();
	function walk(parent: string | null, depth: number) {
		for (const node of nodes.filter((entry) => entry.parent_id === parent).sort(compareNodes)) {
			if (visited.has(node.id)) continue;
			visited.add(node.id); result.push({ ...node, depth });
			if (node.kind === 'folder' && !collapsed.has(node.id)) walk(node.id, depth + 1);
		}
	}
	walk(null, 0); return result;
}
