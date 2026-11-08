import fs from 'node:fs';
const directory = 'state/runs/00-model';
function read(name) {
	const text = fs.readFileSync(`${directory}/${name}.stdout.txt`, 'utf8').replace(/^\uFEFF/, '');
	const segments = JSON.parse(text.slice(text.indexOf('segments=') + 9));
	const metrics = Object.fromEntries([...text.matchAll(/^(\w+)=([\d.]+)$/gm)].map((match) => [match[1], Number(match[2])]));
	return { segments, metrics };
}
const clean = read('clean');
const noisy = read('noisy');
const mismatches = [];
if (clean.segments.length !== noisy.segments.length) mismatches.push('segment count');
for (let i = 0; i < Math.min(clean.segments.length, noisy.segments.length); i++) {
	const a = clean.segments[i]; const b = noisy.segments[i];
	if (Math.abs(a.start_seconds - b.start_seconds) > 0.25 || Math.abs(a.end_seconds - b.end_seconds) > 0.25) mismatches.push(`boundary ${i}`);
}
const result = { scope: 'clean versus low-noise file inference, boundary tolerance 250 ms; not Live', clean: clean.metrics, noisy: noisy.metrics, phrase_coverage: '3/3 each (asserted by model runner)', segment_mismatches: mismatches, status: mismatches.length ? 'failed' : 'verified' };
fs.writeFileSync(`${directory}/consistency.json`, `${JSON.stringify(result, null, 2)}\n`);
console.log(JSON.stringify(result, null, 2));
if (mismatches.length) process.exitCode = 1;
