import { expect, test } from '@playwright/test';

// Synthetic Web Audio exercises browser capture plumbing, NOT speech recognition
// or real screen sharing. Each request owns independent tracks.
test('synthetic microphone/display streams can be captured and stopped independently', async ({ page }) => {
	await page.addInitScript(() => {
		const contexts: AudioContext[] = [];
		const stream = async () => {
			const context = new AudioContext();
			contexts.push(context);
			const oscillator = context.createOscillator();
			const destination = context.createMediaStreamDestination();
			oscillator.connect(destination);
			oscillator.start();
			return destination.stream;
		};
		Object.defineProperty(navigator.mediaDevices, 'getUserMedia', { value: stream });
		Object.defineProperty(navigator.mediaDevices, 'getDisplayMedia', { value: stream });
		addEventListener('pagehide', () => contexts.forEach((context) => void context.close()));
	});
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const mic = await navigator.mediaDevices.getUserMedia({ audio: true });
		const display = await navigator.mediaDevices.getDisplayMedia({ audio: true });
		const microphoneTrack = mic.getAudioTracks()[0];
		const displayTrack = display.getAudioTracks()[0];
		microphoneTrack.stop();
		const independent = displayTrack.readyState;
		displayTrack.stop();
		return { independent, mic: microphoneTrack.readyState, display: displayTrack.readyState };
	});
	expect(result).toEqual({ independent: 'live', mic: 'ended', display: 'ended' });
});
