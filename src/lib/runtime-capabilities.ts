import type { CapabilityMatrix } from './contracts';

export function runtimeCapabilities(desktop: boolean): CapabilityMatrix {
	return {
		system_audio: desktop ? 'available' : 'optional',
		offline_queue: true,
		playback: true,
		browser_local_model: 'disabled',
		remote_inference: 'disabled',
		cloud_sync: 'disabled'
	};
}
