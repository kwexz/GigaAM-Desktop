import type { ArchiveWriter, CaptureSource, RecognitionEngine, TranscriptRepository, SyncProvider, CapabilityMatrix } from './contracts';

/** Runtime-neutral dependency bundle for Svelte routes. */
export type AppPorts = {
	capture: CaptureSource;
	archive: ArchiveWriter;
	recognition: RecognitionEngine;
	repository: TranscriptRepository;
	sync?: SyncProvider;
	capabilities: CapabilityMatrix;
};

/** Shared UI can use this seam without importing @tauri-apps packages. */
export function createUnavailablePorts(reason = 'No runtime adapter installed'): AppPorts {
	const unavailable = async (): Promise<never> => { throw new Error(reason); };
	return {
		capture: {
			start: unavailable, pause: unavailable, resume: unavailable, stop: unavailable,
			onBlock: () => () => {},
		},
		archive: { append: unavailable, finalize: unavailable, abort: unavailable },
		recognition: { kind: 'browser', start: unavailable, cancel: unavailable, onSegment: () => () => {} },
		repository: { getSession: async () => undefined, putSegment: unavailable, changesSince: async () => [] },
		capabilities: {
			system_audio: 'unavailable', offline_queue: false, playback: false,
			browser_local_model: 'disabled', remote_inference: 'disabled', cloud_sync: 'disabled',
		},
	};
}
