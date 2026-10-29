/** Explicit playback/export mix; capture archives remain independent. */
export async function mixBrowserAudio(files: Blob[]): Promise<Blob> {
	if (files.reduce((sum, file) => sum + file.size, 0) > 64 * 1024 * 1024) throw new Error('Audio is too large for in-memory browser mixing');
	const context = new AudioContext();
	try {
		const buffers = await Promise.all(files.map(async (file) => context.decodeAudioData(await file.arrayBuffer())));
		const duration = Math.max(...buffers.map((buffer) => buffer.duration));
		if (duration > 3600) throw new Error('Recording exceeds browser mix limit');
		const rate = 48000;
		const offline = new OfflineAudioContext(1, Math.ceil(duration * rate), rate);
		for (const buffer of buffers) {
			const source = offline.createBufferSource(); source.buffer = buffer;
			const gain = offline.createGain(); gain.gain.value = 1 / buffers.length;
			source.connect(gain).connect(offline.destination); source.start();
		}
		const rendered = await offline.startRendering();
		const samples = rendered.getChannelData(0);
		const bytes = new ArrayBuffer(44 + samples.length * 2); const view = new DataView(bytes);
		const text = (offset: number, value: string) => [...value].forEach((char, i) => view.setUint8(offset + i, char.charCodeAt(0)));
		text(0, 'RIFF'); view.setUint32(4, bytes.byteLength - 8, true); text(8, 'WAVE'); text(12, 'fmt ');
		view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true); view.setUint32(24, rate, true); view.setUint32(28, rate * 2, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true); text(36, 'data'); view.setUint32(40, samples.length * 2, true);
		samples.forEach((sample, index) => view.setInt16(44 + index * 2, Math.max(-1, Math.min(1, sample)) * 32767, true));
		return new Blob([bytes], { type: 'audio/wav' });
	} finally { await context.close(); }
}
