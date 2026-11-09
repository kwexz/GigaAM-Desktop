import { expect, it } from 'vitest';
import { newerStableVersion } from './update-check';

it('compares stable release versions numerically', () => {
	expect(newerStableVersion('0.2.0', 'v0.10.0')).toBe(true);
	expect(newerStableVersion('0.2.0', 'v0.2.0')).toBe(false);
	expect(newerStableVersion('0.2.0', 'v0.3.0-beta')).toBe(false);
	expect(newerStableVersion('dev', 'v0.3.0')).toBe(false);
});
