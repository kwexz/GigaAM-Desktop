import { describe, expect, it } from 'vitest';
import { MockCloudProvider, SyncEngine } from './sync';

const change = (id: string, revision: number, text: string) => ({ change_id: id, session_id: 's', kind: 'segment_upsert' as const, revision, payload: { text } });

describe('logical cloud sync', () => {
	it('push/pull is idempotent and merges independent revisions', async () => {
		const provider = new MockCloudProvider();
		const first = new SyncEngine(provider);
		await first.record(change('a', 1, 'one')); await first.push(); await first.push();
		const second = new SyncEngine(provider);
		expect(await second.pull()).toBe(1);
		expect(second.get('a')?.payload).toEqual({ text: 'one' });
		expect(await first.pull()).toBe(1);
		await second.record(change('b', 1, 'two')); await second.push();
		expect(await first.pull()).toBe(1);
		expect(first.get('b')?.payload).toEqual({ text: 'two' });
	});
	it('same record uses newest revision and supports tombstones', async () => {
		const provider = new MockCloudProvider(); const engine = new SyncEngine(provider);
		await engine.record({ ...change('a', 1, 'old'), deleted: false });
		await engine.record({ ...change('a', 2, ''), deleted: true }); await engine.push();
		expect(engine.get('a')?.deleted).toBe(true);
	});
});
