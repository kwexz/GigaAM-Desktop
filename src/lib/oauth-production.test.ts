import { describe, expect, it, vi } from 'vitest';
import { ProductionOAuth } from './oauth-production';

describe('production OAuth boundary', () => {
	it('uses S256 and single-use bound callback without persisting tokens', async () => {
		const auth = new ProductionOAuth('yandex-disk', { clientId: 'public-client', redirectUri: 'https://app.example/callback' });
		const url = new URL(await auth.begin());
		expect(url.origin).toBe('https://oauth.yandex.ru');
		expect(url.searchParams.get('code_challenge_method')).toBe('S256');
		const exchange = vi.fn(async () => ({ access_token: 'test-token', expires_in: 3600 }));
		const callback = `https://app.example/callback?code=test&state=${url.searchParams.get('state')}`;
		await auth.complete(callback, exchange);
		expect(auth.token()).toBe('test-token');
		expect(exchange.mock.calls).toHaveLength(1);
		await expect(auth.complete(callback, exchange)).rejects.toThrow('pending');
		auth.logout();
		expect(() => auth.token()).toThrow();
	});
	it('rejects wrong redirect and does not call token exchange', async () => {
		const auth = new ProductionOAuth('google-drive', { clientId: 'public-client', redirectUri: 'https://app.example/callback' });
		const url = new URL(await auth.begin());
		const exchange = vi.fn();
		await expect(auth.complete(`https://evil.example/callback?code=x&state=${url.searchParams.get('state')}`, exchange)).rejects.toThrow('callback');
		expect(exchange).not.toHaveBeenCalled();
	});
	it('rejects missing configuration and unsafe redirects', async () => {
		await expect(new ProductionOAuth('google-drive', { clientId: '', redirectUri: 'https://app.example' }).begin()).rejects.toThrow('configured');
		await expect(new ProductionOAuth('google-drive', { clientId: 'id', redirectUri: 'http://remote.example' }).begin()).rejects.toThrow('Unsafe');
	});
});
