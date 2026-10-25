import { expect, test } from '@playwright/test';

test('browser recorder harness archives nonempty audio while another track stops', async ({ page }) => {
	await page.goto('/capture-harness.html');
	const result = await page.evaluate(async () => {
		const context = new AudioContext();
		await context.resume();
		const oscillator = context.createOscillator();
		const microphone = context.createMediaStreamDestination();
		const display = context.createMediaStreamDestination();
		oscillator.connect(microphone);
		oscillator.connect(display);
		oscillator.start();
		const record = (stream: MediaStream) => {
			const chunks: Blob[] = [];
			const recorder = new MediaRecorder(stream);
			const done = new Promise<Blob>((resolve, reject) => {
				recorder.ondataavailable = (event) => chunks.push(event.data);
				recorder.onerror = reject;
				recorder.onstop = () => resolve(new Blob(chunks, { type: recorder.mimeType }));
			});
			recorder.start(50);
			return { recorder, done };
		};
		const mic = record(microphone.stream);
		const sys = record(display.stream);
		await new Promise((resolve) => setTimeout(resolve, 1500));
		mic.recorder.stop();
		const micBlob = await mic.done;
		microphone.stream.getTracks().forEach((track) => track.stop());
		const independentState = sys.recorder.state;
		await new Promise((resolve) => setTimeout(resolve, 500));
		sys.recorder.stop();
		const sysBlob = await sys.done;
		const root = await navigator.storage.getDirectory();
		const name = `capture-test-${crypto.randomUUID()}.webm`;
		try {
			const handle = await root.getFileHandle(name, { create: true });
			const writer = await handle.createWritable();
			await writer.write(sysBlob);
			await writer.close();
			const saved = await handle.getFile();
			return { micBytes: micBlob.size, sysBytes: sysBlob.size, savedBytes: saved.size, independentState };
		} finally {
			await root.removeEntry(name);
			display.stream.getTracks().forEach((track) => track.stop());
			oscillator.stop();
			await context.close();
		}
	});
	expect(result.micBytes).toBeGreaterThan(100);
	expect(result.sysBytes).toBeGreaterThan(100);
	expect(result.savedBytes).toBe(result.sysBytes);
	expect(result.independentState).toBe('recording');
});
