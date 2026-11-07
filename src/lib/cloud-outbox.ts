import { uploadCloudArchive } from './cloud-backup';
import { CloudFiles } from './cloud-files';
import { browserCloudJobs, putBrowserCloudJob, readOpfsArchive, type BrowserCloudJob } from './browser-storage';

/** A failed/interrupted job remains durable and retryable. Authorization is
 * supplied at run time and is never persisted with the job. */
export class BrowserCloudOutbox {
	private running = false;
	async enqueue(provider: BrowserCloudJob['provider'], sessionId: string, archivePath: string): Promise<void> {
		const id = `${provider}:${sessionId}:${archivePath}`;
		const existing = (await browserCloudJobs()).find((job) => job.id === id);
		if (existing?.state === 'synced') return;
		await putBrowserCloudJob({ id, provider, session_id: sessionId, archive_path: archivePath, state: 'pending', attempts: existing?.attempts ?? 0 });
	}
	async drain(provider: CloudFiles): Promise<void> {
		if (this.running) return;
		this.running = true;
		try {
			if (navigator.locks) {
				await navigator.locks.request(`gigaam-cloud-outbox:${provider.provider}`, () => this.process(provider));
			} else {
				throw new Error('Cross-tab cloud synchronization requires Web Locks support');
			}
		} finally { this.running = false; }
	}
	private async process(provider: CloudFiles): Promise<void> {
			for (const job of await browserCloudJobs()) {
				if (job.provider !== provider.provider || job.state === 'synced') continue;
				const active = { ...job, state: 'syncing' as const, attempts: job.attempts + 1 };
				await putBrowserCloudJob(active);
				try {
					const audio = await readOpfsArchive(job.archive_path);
					const track = job.archive_path.split('/').at(-1)?.replace(/\.[^.]+$/, '') ?? 'audio';
					await uploadCloudArchive(provider, `${job.session_id}-${track}`, audio);
					await putBrowserCloudJob({ ...active, state: 'synced' });
				} catch (error) {
					await putBrowserCloudJob({ ...active, state: 'failed' });
					throw error;
				}
			}
	}
}
