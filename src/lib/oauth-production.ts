import type { CloudProviderName, OAuthState } from './cloud-auth';

export type OAuthConfiguration = { clientId: string; redirectUri: string };
type PendingAuthorization = { state: string; verifier: string; created: number; redirect: string };

/** Authorization code + S256 helper. Tokens never enter browser persistent storage.
 * Token exchange is injected so platform adapters can use a provider-supported
 * public-client exchange or a configured trusted exchange, never a bundled secret. */
export class ProductionOAuth {
	state: OAuthState = 'disabled';
	private pending?: PendingAuthorization;
	private accessToken?: string;
	private expiresAt = 0;
	constructor(readonly provider: CloudProviderName, private config: OAuthConfiguration) {}
	async begin(): Promise<string> {
		this.accessToken = undefined;
		this.expiresAt = 0;
		this.pending = undefined;
		if (!this.config.clientId.trim()) throw new Error('OAuth client ID is not configured');
		const redirect = new URL(this.config.redirectUri);
		if (redirect.protocol !== 'https:' && !(redirect.protocol === 'http:' && ['127.0.0.1', 'localhost'].includes(redirect.hostname))) throw new Error('Unsafe OAuth redirect URI');
		const random = () => base64url(crypto.getRandomValues(new Uint8Array(32)));
		const verifier = random();
		const state = random();
		const challenge = base64url(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier))));
		this.pending = { state, verifier, created: Date.now(), redirect: redirect.href };
		this.state = 'authorizing';
		const url = new URL(this.provider === 'google-drive' ? 'https://accounts.google.com/o/oauth2/v2/auth' : 'https://oauth.yandex.ru/authorize');
		url.search = new URLSearchParams({ client_id: this.config.clientId, redirect_uri: redirect.href, response_type: 'code', state, code_challenge: challenge, code_challenge_method: 'S256', scope: this.provider === 'google-drive' ? 'https://www.googleapis.com/auth/drive.appdata' : 'cloud_api:app_folder' }).toString();
		return url.href;
	}
	async complete(callback: string, exchange: (code: string, verifier: string, redirect: string) => Promise<{ access_token: string; expires_in: number }>): Promise<void> {
		const pending = this.pending;
		this.pending = undefined; // Single use even on failure.
		if (!pending) throw new Error('No pending OAuth authorization');
		let url: URL;
		try { url = new URL(callback); } catch { this.state = 'disabled'; throw new Error('Invalid OAuth callback'); }
		const redirect = new URL(pending.redirect);
		if (url.origin !== redirect.origin || url.pathname !== redirect.pathname || url.username || url.password || url.hash || url.searchParams.getAll('state').length !== 1 || url.searchParams.getAll('code').length > 1 || url.searchParams.get('state') !== pending.state || Date.now() - pending.created > 10 * 60_000) {
			this.state = 'disabled';
			throw new Error('Invalid or expired OAuth callback');
		}
		const code = url.searchParams.get('code');
		if (!code || url.searchParams.has('error')) { this.state = 'disabled'; throw new Error('Authorization was not granted'); }
		try {
			const token = await exchange(code, pending.verifier, pending.redirect);
			if (!token.access_token || !Number.isFinite(token.expires_in) || token.expires_in <= 0) throw new Error('Invalid token response');
			this.accessToken = token.access_token;
			this.expiresAt = Date.now() + token.expires_in * 1000;
			this.state = 'authorized';
		} catch { this.state = 'disabled'; throw new Error('OAuth token exchange failed'); }
	}
	token(): string {
		if (Date.now() >= this.expiresAt) { this.accessToken = undefined; this.state = 'expired'; }
		if (!this.accessToken) throw new Error('Cloud authorization required');
		return this.accessToken;
	}
	logout(): void { this.accessToken = undefined; this.pending = undefined; this.expiresAt = 0; this.state = 'revoked'; }
}

function base64url(bytes: Uint8Array): string {
	return btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
