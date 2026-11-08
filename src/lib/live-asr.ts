/** Bounded, independent live inference consumer. It never gates archive writes. */
export class LiveAsr {
	private context?: AudioContext;
	private processors: ScriptProcessorNode[] = [];
	private windows = new Map<string, { samples: Float32Array[]; count: number; start: number }>();
	private queue: Array<{ track: string; samples: Float32Array; start: number; rate: number }> = [];
	private running?: Promise<void>;
	private stopped = false;
	private paused = false;
	pause(): void { this.paused = true; }
	resume(): void { this.paused = false; }
	onResult?: (track: string, start: number, segments: Array<{ start_seconds: number; end_seconds: number; text: string }>) => Promise<void>;
	onError?: (message: string) => void;
	attach(track: string, stream: MediaStream, offset: number): void {
		this.context ??= new AudioContext();
		void this.context.resume();
		const state = { samples: [] as Float32Array[], count: 0, start: offset };
		this.windows.set(track, state);
		const source = this.context.createMediaStreamSource(stream);
		const processor = this.context.createScriptProcessor(4096, 1, 1);
		processor.onaudioprocess = (event) => {
			if (this.stopped || this.paused) return;
			const samples = event.inputBuffer.getChannelData(0).slice();
			state.samples.push(samples); state.count += samples.length;
			if (state.count >= this.context!.sampleRate * 8) this.flush(track);
		};
		const mute = this.context.createGain(); mute.gain.value = 0;
		source.connect(processor).connect(mute).connect(this.context.destination);
		this.processors.push(processor);
	}
	private flush(track: string): void {
		const state = this.windows.get(track)!;
		if (!state.count) return;
		const samples = new Float32Array(state.count); let offset = 0;
		for (const block of state.samples) { samples.set(block, offset); offset += block.length; }
		const start = state.start; state.start += state.count / this.context!.sampleRate;
		state.samples = []; state.count = 0;
		if (this.queue.length >= 8) { this.onError?.('Live ASR backlog full; audio is still archived locally. Retry recognition after Stop.'); return; }
		this.queue.push({ track, samples, start, rate: this.context!.sampleRate });
		this.running ??= this.drain().finally(() => { this.running = undefined; });
	}
	private async drain(): Promise<void> {
		while (this.queue.length) {
			const job = this.queue.shift()!;
			try {
				const bytes = new ArrayBuffer(44 + job.samples.length * 2), view = new DataView(bytes);
				const text = (o: number, s: string) => [...s].forEach((c, i) => view.setUint8(o + i, c.charCodeAt(0)));
				text(0,'RIFF');view.setUint32(4,bytes.byteLength-8,true);text(8,'WAVE');text(12,'fmt ');view.setUint32(16,16,true);view.setUint16(20,1,true);view.setUint16(22,1,true);view.setUint32(24,job.rate,true);view.setUint32(28,job.rate*2,true);view.setUint16(32,2,true);view.setUint16(34,16,true);text(36,'data');view.setUint32(40,job.samples.length*2,true);
				job.samples.forEach((value,i)=>view.setInt16(44+i*2,Math.max(-1,Math.min(1,value))*32767,true));
				let response: Response | undefined;
				for (let attempt=0;attempt<4;attempt++) {
					response=await fetch('/api/v1/transcribe',{method:'POST',body:new Blob([bytes],{type:'audio/wav'})});
					if(response.status!==429)break;
					await new Promise((resolve)=>setTimeout(resolve,500*(attempt+1)));
				}
				if (!response?.ok) throw new Error(`Live ASR HTTP ${response?.status}`);
				await this.onResult?.(job.track, job.start, await response.json());
			} catch (error) { this.onError?.(String(error)); }
		}
	}
	async stop(): Promise<void> {
		this.stopped = true;
		for (const processor of this.processors) { processor.onaudioprocess = null; processor.disconnect(); }
		for (const track of this.windows.keys()) this.flush(track);
		await this.running;
		await this.context?.close();
	}
}
