export type CloudProviderName = 'google-drive' | 'yandex-disk';
export type OAuthState = 'disabled' | 'authorizing' | 'authorized' | 'expired' | 'revoked';

export class MockOAuthProvider {
	readonly state = { google: 'disabled' as OAuthState, yandex: 'disabled' as OAuthState };
	private verifier?: string;
	begin(provider: CloudProviderName): { state: OAuthState; authorization_url: string } {
		this.verifier = crypto.randomUUID();
		this.state[provider === 'google-drive' ? 'google' : 'yandex'] = 'authorizing';
		return { state: 'authorizing', authorization_url: `mock://${provider}/authorize?code_challenge=${this.verifier}` };
	}
	complete(provider: CloudProviderName, code: string): void {
		if (!this.verifier || !code) throw new Error('invalid PKCE callback');
		this.state[provider === 'google-drive' ? 'google' : 'yandex'] = 'authorized';
	}
	logout(provider: CloudProviderName): void { this.state[provider === 'google-drive' ? 'google' : 'yandex'] = 'revoked'; this.verifier = undefined; }
}
