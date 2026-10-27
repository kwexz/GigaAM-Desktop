import { describe, expect, it } from 'vitest';
import { WebSession } from './web-session';

describe('WebSession', () => {
	it('starts in explicit local-first state and keeps remote optional', () => {
		const session = new WebSession('00000000-0000-4000-8000-000000000001');
		expect(session.status()).toEqual({ recording: 'idle', upload: 'not_started', recognition: 'not_started', sync: 'disabled' });
		expect(session.remote).toBeUndefined();
	});
});
