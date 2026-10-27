import { describe, expect, it } from 'vitest';
import { BrowserAsrEngine } from './browser-asr';

describe('BrowserAsrEngine opt-in gate', () => {
	it('never auto-downloads or starts without verified model', async () => {
		const engine = new BrowserAsrEngine();
		expect(engine.status()).toEqual({ state: 'disabled', reason: 'opt_in_required' });
		await expect(engine.start('00000000-0000-4000-8000-000000000001')).rejects.toThrow('verified model');
	});
	it('rejects a manifest with a mismatched checksum', () => {
		const engine = new BrowserAsrEngine();
		engine.installVerifiedModel({ version: 'v3', bytes: 227_000_000, sha256: 'actual' }, 'expected');
		expect(engine.status().state).toBe('blocked');
	});
});
