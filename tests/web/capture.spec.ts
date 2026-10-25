import { expect, test } from '@playwright/test';

// Synthetic Web Audio exercises browser capture plumbing, NOT speech recognition
// or real screen sharing. Each request owns independent tracks.
test('synthetic microphone/display streams can be captured and stopped independently', async ({ page }) => {
	await page.addInitScript(() => {
		const contexts: AudioContext[] = [];
		const stream = async () => {
			const context = new AudioContext();
			await context.resume();
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
	await page.goto('/capture-harness.html');
	const result = await page.evaluate(async () => {
		const mic = await navigator.mediaDevices.getUserMedia({ audio: true });
		const display = await navigator.mediaDevices.getDisplayMedia({ audio: true });
		const microphoneTrack = mic.getAudioTracks()[0];
		const displayTrack = display.getAudioTracks()[0];
		const recorder = new MediaRecorder(mic);
		const chunks: Blob[] = [];
		const recorded = new Promise<Blob>((resolve) => {
			recorder.ondataavailable = (event) => chunks.push(event.data);
			recorder.onstop = () => resolve(new Blob(chunks, { type: recorder.mimeType }));
		});
		recorder.start();
		await new Promise((resolve) => setTimeout(resolve, 1500));
		recorder.stop();
		const blob = await recorded;
		microphoneTrack.stop();
		const independent = displayTrack.readyState;
		displayTrack.stop();
		return { independent, mic: microphoneTrack.readyState, display: displayTrack.readyState, bytes: blob.size };
	});
	expect(result.bytes).toBeGreaterThan(100);
	expect(result).toMatchObject({ independent: 'live', mic: 'ended', display: 'ended' });
});
