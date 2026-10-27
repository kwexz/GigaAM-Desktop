import { describe, expect, it } from 'vitest';
import { WebSession } from './web-session';

describe('WebSession', () => {
	it('starts in explicit local-first state and keeps remote optional', () => {
		const session = new WebSession('test-session');
		expect(session.status()).toEqual({ recording: 'idle', upload: 'not_started', recognition: 'not_started', sync: 'disabled' });
		expect(session.remote).toBeUndefined();
	});
});
