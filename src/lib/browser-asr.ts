import type { RecognitionEngine, TranscriptSegmentEvent } from './contracts';

export type BrowserAsrStatus =
	| { state: 'disabled'; reason: 'opt_in_required' | 'model_not_installed' }
	| { state: 'ready'; model_version: string; model_bytes: number }
	| { state: 'blocked'; reason: string };

/**
 * Deliberately gated browser ASR adapter. It never fetches model weights from
 * a constructor or from `start`; an installer must provide a verified OPFS
 * model manifest before this engine can be enabled.
 */
export class BrowserAsrEngine implements RecognitionEngine {
	readonly kind = 'browser' as const;
	private listeners = new Set<(event: TranscriptSegmentEvent) => void>();
	private statusValue: BrowserAsrStatus = { state: 'disabled', reason: 'opt_in_required' };

	status(): BrowserAsrStatus { return this.statusValue; }
	installVerifiedModel(manifest: { version: string; bytes: number; sha256: string }, verifiedSha256: string): void {
		if (!manifest.version || manifest.bytes <= 0 || manifest.sha256 !== verifiedSha256) {
			this.statusValue = { state: 'blocked', reason: 'model manifest verification failed' };
			return;
		}
		this.statusValue = { state: 'ready', model_version: manifest.version, model_bytes: manifest.bytes };
	}
	async start(_sessionId: string): Promise<void> {
		if (this.statusValue.state !== 'ready') throw new Error('browser ASR requires explicit verified model installation');
		throw new Error('browser ONNX compatibility is not validated; remote engine remains the fallback');
	}
	async cancel(_sessionId: string): Promise<void> {}
	onSegment(listener: (event: TranscriptSegmentEvent) => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
}
