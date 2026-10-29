/** Convert browser-supported recording containers to the server's PCM input.
 * Original OPFS archives are never modified. */
export async function prepareAsrUpload(file: Blob): Promise<Blob> {
	if (file.size > 64 * 1024 * 1024) throw new Error('Recording exceeds browser ASR conversion limit');
	const context = new AudioContext({ sampleRate: 16000 });
	try {
		const decoded = await context.decodeAudioData(await file.arrayBuffer());
		if (decoded.duration > 3600) throw new Error('Recording exceeds ASR conversion duration limit');
		const offline = new OfflineAudioContext(1, Math.ceil(decoded.duration * 16000), 16000);
		const source = offline.createBufferSource(); source.buffer = decoded; source.connect(offline.destination); source.start();
		const samples = (await offline.startRendering()).getChannelData(0);
		const bytes = new ArrayBuffer(44 + samples.length * 2), view = new DataView(bytes);
		const text = (offset: number, value: string) => [...value].forEach((char, index) => view.setUint8(offset + index, char.charCodeAt(0)));
		text(0, 'RIFF'); view.setUint32(4, bytes.byteLength - 8, true); text(8, 'WAVE'); text(12, 'fmt '); view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true); view.setUint32(24, 16000, true); view.setUint32(28, 32000, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true); text(36, 'data'); view.setUint32(40, samples.length * 2, true);
		samples.forEach((sample, index) => view.setInt16(44 + index * 2, Math.max(-1, Math.min(1, sample)) * 32767, true));
		return new Blob([bytes], { type: 'audio/wav' });
	} finally { await context.close(); }
}
