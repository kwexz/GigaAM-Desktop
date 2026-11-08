import { expect, test } from '@playwright/test';

test('browser WebM capture is normalized to PCM WAV before ASR upload', async ({ page }) => {
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const { prepareAsrUpload } = await import('/src/lib/asr-upload.ts');
		const context = new AudioContext(); await context.resume();
		const oscillator = context.createOscillator(), output = context.createMediaStreamDestination();
		oscillator.connect(output); oscillator.start();
		const chunks: Blob[] = []; const recorder = new MediaRecorder(output.stream);
		recorder.ondataavailable = (event) => chunks.push(event.data);
		const stopped = new Promise<void>((resolve) => { recorder.onstop = () => resolve(); });
		recorder.start(); await new Promise((resolve) => setTimeout(resolve, 500)); recorder.stop(); await stopped;
		const wav = await prepareAsrUpload(new Blob(chunks, { type: recorder.mimeType }));
		const bytes = await wav.arrayBuffer(); const view = new DataView(bytes);
		oscillator.stop(); output.stream.getTracks().forEach((track) => track.stop()); await context.close();
		return { magic: String.fromCharCode(...new Uint8Array(bytes, 0, 4)), rate: view.getUint32(24, true), channels: view.getUint16(22, true), bytes: bytes.byteLength };
	});
	expect(result.magic).toBe('RIFF'); expect(result.rate).toBe(16000); expect(result.channels).toBe(1); expect(result.bytes).toBeGreaterThan(1000);
});
