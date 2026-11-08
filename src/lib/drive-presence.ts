/** A matching uploaded revision is evidence of a cloud copy, not proof that
 * subsequent local edits have been synchronized. Never infer identity by name. */
export function drivePresence(
	lineage: { original_id: string; revision: string } | undefined,
	branches: ReadonlyArray<{ original_id: string; revision: string; concurrent: boolean }>,
	discoveryComplete: boolean
): 'unknown' | 'revision-found' | 'conflict' {
	// Older restores may contain a synthetic "legacy" revision rather than a
	// provider-backed identity. It must never be evidence of a matching snapshot.
	if (!discoveryComplete || !lineage || !lineage.original_id.trim() || !lineage.revision.trim() || lineage.revision === 'legacy') return 'unknown';
	const related = branches.filter((branch) => branch.original_id === lineage.original_id);
	if (related.some((branch) => branch.concurrent)) return 'conflict';
	// Even a complete listing can become stale; absence is not evidence of the
	// current remote state. A matching snapshot is not proof of local freshness.
	return related.some((branch) => branch.revision === lineage.revision) ? 'revision-found' : 'unknown';
}
