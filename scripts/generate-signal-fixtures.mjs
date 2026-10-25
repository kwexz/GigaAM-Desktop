import { mkdirSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const directory = 'tests/fixtures';
mkdirSync(directory, { recursive: true });
const manifest = { provenance: 'Original deterministic mathematical signals; no recorded voices or private audio', license: 'CC0-1.0', sampleRate: 16000, files: [] };
function save(name, bytes, description) {
	writeFileSync(`${directory}/${name}`, bytes);
	manifest.files.push({ name, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), description });
}
function wav(seconds, channels) {
	const frames = 16000 * seconds;
	const data = Buffer.alloc(44 + frames * channels * 2);
	data.write('RIFF'); data.writeUInt32LE(data.length - 8, 4); data.write('WAVEfmt ', 8);
	data.writeUInt32LE(16, 16); data.writeUInt16LE(1, 20); data.writeUInt16LE(channels, 22);
	data.writeUInt32LE(16000, 24); data.writeUInt32LE(16000 * channels * 2, 28);
	data.writeUInt16LE(channels * 2, 32); data.writeUInt16LE(16, 34);
	data.write('data', 36); data.writeUInt32LE(data.length - 44, 40);
	for (let frame = 0; frame < frames; frame++) {
		for (let channel = 0; channel < channels; channel++) {
			const active = frame % 32000 < 16000;
			const sample = active ? Math.round(3000 * Math.sin(2 * Math.PI * (440 + channel * 220) * frame / 16000)) : 0;
			data.writeInt16LE(sample, 44 + (frame * channels + channel) * 2);
		}
	}
	return data;
}
save('dual-tones.wav', wav(6, 2), 'Stereo 440/660 Hz signals with one-second pauses; not speech');
save('corrupt.wav', Buffer.from('not a valid wave file\n'), 'Intentionally invalid audio');
// Long input is generated on demand, not committed as a large binary.
if (process.argv.includes('--long')) save('long-tones.wav', wav(600, 1), 'Ten-minute bounded-memory/timeline test signal; not speech');
writeFileSync(`${directory}/signals.json`, `${JSON.stringify(manifest, null, 2)}\n`);
