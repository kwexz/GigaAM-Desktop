import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';

test('real Linux ASR returns speech while the live source is still recording', async ({ page }) => {
	test.skip(process.env.GIGAAM_REAL_ASR !== '1', 'Explicit real container/model test');
	test.setTimeout(120000);
	await page.goto('/');
	await page.route('**/api/v1/transcribe', async (route) => {
		const response = await page.request.post('http://127.0.0.1:8080/api/v1/transcribe', { data: route.request().postDataBuffer()!, headers: { 'Content-Type': 'audio/wav' } });
		await route.fulfill({ response });
	});
	const bytes = [...readFileSync('tests/fixtures/besarab-clean.wav')];
	const result = await page.evaluate(async (input) => {
		const { LiveAsr } = await import('/src/lib/live-asr.ts');
		const context = new AudioContext(); await context.resume();
		const buffer = await context.decodeAudioData(new Uint8Array(input).buffer);
		const source = context.createBufferSource(); source.buffer = buffer;
		const destination = context.createMediaStreamDestination(); source.connect(destination);
		const live = new LiveAsr();
		let count = 0;
		let finished = false; source.onended = () => { finished = true; };
		const response = new Promise<{ count: number; beforeEnd: boolean }>((resolve, reject) => {
			const timeout = setTimeout(() => reject(new Error('No live transcript')), 60000);
			live.onResult = async (_track, _start, segments) => { count += segments.length; if (count) { clearTimeout(timeout); resolve({ count, beforeEnd: !finished }); } };
			live.onError = (message) => { clearTimeout(timeout); reject(new Error(message)); };
		});
		live.attach('microphone', destination.stream, 0); source.start();
		try { return await response; }
		finally { source.stop(); await live.stop(); await context.close(); }
	}, bytes);
	expect(result.count).toBeGreaterThan(0);
	expect(result.beforeEnd).toBe(true);
});
