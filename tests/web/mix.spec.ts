import { expect, test } from '@playwright/test';

test('browser mix includes both audio sources in a playable WAV', async ({ page }) => {
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const { mixBrowserAudio } = await import('/src/lib/browser-mix.ts');
		function wav(frequency: number) {
			const rate = 48000, frames = rate;
			const bytes = new ArrayBuffer(44 + frames * 2), view = new DataView(bytes);
			const text = (offset: number, value: string) => [...value].forEach((c, i) => view.setUint8(offset + i, c.charCodeAt(0)));
			text(0, 'RIFF'); view.setUint32(4, bytes.byteLength - 8, true); text(8, 'WAVE'); text(12, 'fmt '); view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true); view.setUint32(24, rate, true); view.setUint32(28, rate * 2, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true); text(36, 'data'); view.setUint32(40, frames * 2, true);
			for (let i = 0; i < frames; i++) view.setInt16(44 + i * 2, Math.sin(i * frequency * 2 * Math.PI / rate) * 10000, true);
			return new Blob([bytes], { type: 'audio/wav' });
		}
		const mixed = await mixBrowserAudio([wav(440), wav(880)]);
		const context = new AudioContext();
		const buffer = await context.decodeAudioData(await mixed.arrayBuffer());
		const samples = buffer.getChannelData(0);
		const amplitude = (frequency: number) => Math.abs(samples.reduce((sum, sample, i) => sum + sample * Math.sin(i * frequency * 2 * Math.PI / buffer.sampleRate), 0) / samples.length);
		const output = { duration: buffer.duration, a: amplitude(440), b: amplitude(880) };
		await context.close(); return output;
	});
	expect(result.duration).toBeCloseTo(1, 2);
	expect(result.a).toBeGreaterThan(0.02);
	expect(result.b).toBeGreaterThan(0.02);
});
