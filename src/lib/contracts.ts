/**
 * Versioned cross-runtime contracts. The Svelte UI depends on these ports,
 * never on Tauri imports; the desktop adapter is one implementation.
 */
export const CONTRACT_VERSION = 1 as const;

export type TrackId = 'microphone' | 'system' | 'file';
export type RecognitionEngineKind = 'native' | 'remote' | 'browser';

export type AudioBlockEvent = {
	version: typeof CONTRACT_VERSION;
	session_id: string;
	track_id: TrackId;
	seq: number;
	sample_rate: number;
	absolute_sample_start: number;
	duration_samples: number;
	codec: 'pcm_f32le' | 'opus';
	codec_config?: string;
	flags: { final: boolean; discontinuity: boolean };
	idempotency_key: string;
	payload?: Uint8Array;
};

export type TranscriptSegmentEvent = {
	version: typeof CONTRACT_VERSION;
	session_id: string;
	segment_id: string;
	track_id: TrackId;
	seq: number;
	start_seconds: number;
	end_seconds: number;
	text: string;
	final: boolean;
	revision: number;
	idempotency_key: string;
};

export type RecordingState = 'idle' | 'recording' | 'paused' | 'stopping' | 'completed' | 'failed';
export type UploadState = 'not_started' | 'queued' | 'uploading' | 'durable' | 'failed';
export type RecognitionState = 'not_started' | 'running' | 'completed' | 'cancelled' | 'failed';
export type SyncState = 'disabled' | 'idle' | 'pending' | 'syncing' | 'synced' | 'conflict' | 'failed';

export type SessionState = {
	recording: RecordingState;
	upload: UploadState;
	recognition: RecognitionState;
	sync: SyncState;
};

export interface CaptureSource {
	start(sessionId: string): Promise<void>;
	pause(): Promise<void>;
	resume(): Promise<void>;
	stop(): Promise<void>;
	onBlock(listener: (event: AudioBlockEvent) => void): () => void;
}

export interface ArchiveWriter {
	append(event: AudioBlockEvent): Promise<void>;
	finalize(sessionId: string): Promise<{ content_hash: string; duration_samples: number }>;
	abort(sessionId: string): Promise<void>;
}

export interface RecognitionEngine {
	readonly kind: RecognitionEngineKind;
	start(sessionId: string): Promise<void>;
	cancel(sessionId: string): Promise<void>;
	onSegment(listener: (event: TranscriptSegmentEvent) => void): () => void;
}

export interface TranscriptRepository {
	getSession(sessionId: string): Promise<SessionState | undefined>;
	putSegment(event: TranscriptSegmentEvent): Promise<void>;
	changesSince(cursor: string | null): Promise<TranscriptChange[]>;
}

export type TranscriptChange = {
	change_id: string;
	session_id: string;
	kind: 'segment_upsert' | 'segment_delete' | 'session_update';
	revision: number;
	payload: unknown;
};

export interface SyncProvider {
	changesSince(cursor: string | null): Promise<{ cursor: string; changes: TranscriptChange[] }>;
	applyChange(change: TranscriptChange): Promise<void>;
}

export type CapabilityMatrix = {
	system_audio: 'available' | 'optional' | 'unavailable';
	offline_queue: boolean;
	playback: boolean;
	browser_local_model: 'disabled' | 'opt_in' | 'enabled';
	remote_inference: 'disabled' | 'enabled';
	cloud_sync: 'disabled' | 'enabled';
};

export function assertAudioBlock(event: AudioBlockEvent): void {
	if (event.version !== CONTRACT_VERSION) throw new Error('unsupported contract version');
	if (!event.session_id || !event.idempotency_key) throw new Error('missing identity');
	if (!Number.isInteger(event.seq) || event.seq < 0) throw new Error('invalid sequence');
	if (!Number.isInteger(event.sample_rate) || event.sample_rate <= 0) throw new Error('invalid sample rate');
	if (!Number.isInteger(event.absolute_sample_start) || event.absolute_sample_start < 0) throw new Error('invalid sample start');
	if (!Number.isInteger(event.duration_samples) || event.duration_samples <= 0) throw new Error('invalid duration');
}

export function assertSegment(event: TranscriptSegmentEvent): void {
	if (event.version !== CONTRACT_VERSION) throw new Error('unsupported contract version');
	if (!event.session_id || !event.segment_id || !event.idempotency_key) throw new Error('missing segment identity');
	if (!Number.isFinite(event.start_seconds) || !Number.isFinite(event.end_seconds) || event.end_seconds <= event.start_seconds) {
		throw new Error('invalid segment timestamps');
	}
	if (!Number.isInteger(event.revision) || event.revision < 0) throw new Error('invalid revision');
}
