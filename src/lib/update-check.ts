/** Compare stable release versions without treating prereleases as newer. */
export function newerStableVersion(current: string, tag: string): boolean {
	const parse = (value: string) => /^v?(\d+)\.(\d+)\.(\d+)$/.exec(value)?.slice(1).map(Number);
	const left = parse(current), right = parse(tag);
	if (!left || !right) return false;
	for (let i = 0; i < 3; i++) { if (right[i] !== left[i]) return right[i] > left[i]; }
	return false;
}
