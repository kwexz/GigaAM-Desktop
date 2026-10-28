import { expect, test } from '@playwright/test';

test('IndexedDB v1 upgrade preserves existing recording identity and metadata', async ({ page }) => {
	await page.goto('/capture-harness.html');
	await page.evaluate(async () => {
		const root = await navigator.storage.getDirectory();
		const audio = await root.getFileHandle('legacy-audio.webm', { create: true });
		const writer = await audio.createWritable();
		await writer.write(new Uint8Array([1, 2, 3, 4])); await writer.close();
		await new Promise<void>((resolve, reject) => {
			const request = indexedDB.open('gigaam-desktop-browser-v1', 1);
			request.onupgradeneeded = () => {
				const db = request.result;
				db.createObjectStore('metadata', { keyPath: 'key' });
				db.createObjectStore('segments', { keyPath: ['session_id', 'segment_id'] });
				const changes = db.createObjectStore('changes', { keyPath: 'change_id' });
				changes.createIndex('session_revision', ['session_id', 'revision']);
			};
			request.onerror = () => reject(request.error);
			request.onsuccess = () => {
				const db = request.result;
				const tx = db.transaction('metadata', 'readwrite');
				tx.objectStore('metadata').put({ key: 'recording:legacy-id', id: 'legacy-id', name: 'Legacy recording', created_ms: 1, duration_seconds: 10 });
				tx.objectStore('metadata').put({ key: 'archive:legacy-id:microphone', session_id: 'legacy-id', track_id: 'microphone', path: 'legacy-audio.webm', mime_type: 'audio/webm', bytes: 4, block_count: 1, created_ms: 1 });
				tx.oncomplete = () => { db.close(); resolve(); };
			};
		});
	});
	await page.goto('/');
	await expect(page.getByTestId('recording-item')).toContainText('Legacy recording');
	const migrated = await page.evaluate(async () => {
		const { listBrowserMetadata } = await import('/src/lib/browser-storage.ts');
		return (await listBrowserMetadata()).find((entry) => entry.key === 'recording:legacy-id');
	});
	expect(migrated).toMatchObject({ id: 'legacy-id', name: 'Legacy recording', duration_seconds: 10 });
	const preservedAudio = await page.evaluate(async () => {
		const { readOpfsArchive } = await import('/src/lib/browser-storage.ts');
		return Array.from(new Uint8Array(await (await readOpfsArchive('legacy-audio.webm')).arrayBuffer()));
	});
	expect(preservedAudio).toEqual([1, 2, 3, 4]);
});

test('manual order persists through search and reload', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(async () => {
		const { putBrowserRecording } = await import('/src/lib/browser-storage.ts');
		await putBrowserRecording({ id: 'order-a', name: 'Alpha', created_ms: 1, duration_seconds: 1 });
		await putBrowserRecording({ id: 'order-b', name: 'Beta', created_ms: 2, duration_seconds: 1 });
	});
	await page.reload();
	await expect(page.getByTestId('recording-item').first()).toContainText('Alpha');
	await page.getByRole('button', { name: 'Запись ниже', exact: true }).first().click();
	await expect(page.getByTestId('recording-item').first()).toContainText('Beta');
	await page.locator('.search input').fill('Alpha');
	await expect(page.getByTestId('recording-item')).toHaveCount(1);
	await page.locator('.search input').fill('');
	await expect(page.getByTestId('recording-item').first()).toContainText('Beta');
	await page.reload();
	await expect(page.getByTestId('recording-item').first()).toContainText('Beta');
});

test('organization transactions preserve independent concurrent moves', async ({ page }) => {
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const { saveBrowserOrganization, updateBrowserOrganization, loadBrowserOrganization } = await import('/src/lib/browser-storage.ts');
		const { moveNode } = await import('/src/lib/organization.ts');
		await saveBrowserOrganization([
			{ id: 'folder', kind: 'folder', parent_id: null, name: 'Folder', position: 0 },
			{ id: 'a', kind: 'recording', parent_id: null, name: 'A', position: 1 },
			{ id: 'b', kind: 'recording', parent_id: null, name: 'B', position: 2 }
		]);
		await Promise.all([
			updateBrowserOrganization((nodes) => moveNode(nodes, 'a', 'folder', 0)),
			updateBrowserOrganization((nodes) => moveNode(nodes, 'b', 'folder', 1))
		]);
		return loadBrowserOrganization();
	});
	expect(result.find((node) => node.id === 'a')?.parent_id).toBe('folder');
	expect(result.find((node) => node.id === 'b')?.parent_id).toBe('folder');
});

test('transcript replay cannot overwrite a newer local edit', async ({ page }) => {
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const { putSegment, browserSegments, changesSince } = await import('/src/lib/browser-storage.ts');
		const event = { version: 1 as const, session_id: 'session', segment_id: 'segment', track_id: 'microphone' as const, seq: 0, start_seconds: 0, end_seconds: 1, text: 'edited', final: true, revision: 2, idempotency_key: 'new' };
		await putSegment(event);
		await putSegment({ ...event, revision: 1, text: 'old', idempotency_key: 'old' });
		return { segments: await browserSegments('session'), changes: await changesSince('session', -1) };
	});
	expect(result.segments).toHaveLength(1);
	expect(result.segments[0].text).toBe('edited');
	expect(result.changes).toHaveLength(1);
});

test('recording deletion removes local audio and transcript without Desktop IPC', async ({ page }) => {
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const storage = await import('/src/lib/browser-storage.ts');
		const path = 'sessions/delete-test/microphone.webm';
		await storage.appendOpfsChunk(path, new Blob(['audio']));
		await storage.putArchiveMeta({ session_id: 'delete-test', track_id: 'microphone', path, mime_type: 'audio/webm', bytes: 5, block_count: 1, created_ms: 1 });
		await storage.putBrowserRecording({ id: 'delete-test', name: 'Delete', created_ms: 1, duration_seconds: 1 });
		await storage.deleteBrowserRecording('delete-test');
		let exists = true;
		try { await storage.readOpfsArchive(path); } catch { exists = false; }
		return { exists, rows: (await storage.listBrowserMetadata()).filter((entry) => entry.session_id === 'delete-test' || entry.id === 'delete-test') };
	});
	expect(result.exists).toBe(false);
	expect(result.rows).toEqual([]);
});

test('another tab cannot delete an active recording', async ({ page }) => {
	await page.goto('/');
	const rejected = await page.evaluate(async () => {
		const { deleteBrowserRecording } = await import('/src/lib/browser-storage.ts');
		return navigator.locks.request('gigaam-recording:active-session', async () => {
			try { await deleteBrowserRecording('active-session'); return false; }
			catch (error) { return String(error).includes('active in another tab'); }
		});
	});
	expect(rejected).toBe(true);
});

test('interrupted session recovers durable audio without claiming completion', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(async () => {
		const s = await import('/src/lib/browser-storage.ts');
		await s.appendOpfsChunk('sessions/crashed/microphone.webm', new Blob(['saved-before-crash']));
		await s.putArchiveMeta({ session_id: 'crashed', track_id: 'microphone', path: 'sessions/crashed/microphone.webm', mime_type: 'audio/webm', bytes: 18, block_count: 1, created_ms: 1 });
		await s.putBrowserRecording({ id: 'crashed', name: 'Interrupted recording', created_ms: 1, duration_seconds: 0 });
		await s.putSessionState('crashed', { recording: 'recording', upload: 'queued', recognition: 'not_started', sync: 'disabled' });
	});
	await page.reload();
	await expect(page.getByTestId('recording-item')).toContainText('Interrupted recording');
	const recovery = await page.evaluate(async () => {
		const s = await import('/src/lib/browser-storage.ts');
		return { status: await s.browserRecordingStatus('crashed'), audio: await (await s.readOpfsArchive('sessions/crashed/microphone.webm')).text() };
	});
	expect(recovery).toEqual({ status: 'interrupted', audio: 'saved-before-crash' });
});

test('durable block replay preserves sequence and bytes and rejects gaps', async ({ page }) => {
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const { OpfsArchiveWriter } = await import('/src/lib/browser-recorder.ts');
		const { replayArchivedBlocks } = await import('/src/lib/archive-replay.ts');
		const writer = new OpfsArchiveWriter();
		const block = (seq: number) => ({ version: 1 as const, session_id: 'replay', track_id: 'microphone' as const, seq, sample_rate: 48000, absolute_sample_start: seq * 480, duration_samples: 480, codec: 'opus' as const, codec_config: 'audio/webm;codecs=opus', flags: { final: false, discontinuity: false }, idempotency_key: `replay:microphone:${seq}`, payload: new Uint8Array([seq + 1]) });
		await writer.append(block(0)); await writer.append(block(0)); await writer.append(block(1));
		let gapRejected = false;
		try { await writer.append(block(3)); } catch { gapRejected = true; }
		const rows = [];
		for await (const event of replayArchivedBlocks('replay')) rows.push({ seq: event.seq, bytes: Array.from(event.payload!) });
		return { rows, gapRejected };
	});
	expect(result).toEqual({ rows: [{ seq: 0, bytes: [1] }, { seq: 1, bytes: [2] }], gapRejected: true });
});

test('persisted replay reconnects and waits for durable mock-server ACK', async ({ page }) => {
	let connections = 0;
	const received: number[] = [];
	await page.routeWebSocket(/\/v1\/sessions\/reconnect\/live$/, (socket) => {
		connections += 1;
		let pending: { seq: number; track_id: string } | undefined;
		socket.onMessage((message) => {
			if (typeof message === 'string') { pending = JSON.parse(message); return; }
			if (!pending) return;
			received.push(pending.seq);
			if (connections === 1) { socket.close({ code: 1011, reason: 'synthetic disconnect before ACK' }); return; }
			socket.send(JSON.stringify({ type: 'ack', durable: true, track_id: pending.track_id, seq: pending.seq }));
		});
	});
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const { OpfsArchiveWriter } = await import('/src/lib/browser-recorder.ts');
		const { replayToRemote } = await import('/src/lib/archive-replay.ts');
		const { RemoteEngine } = await import('/src/lib/remote-engine.ts');
		const writer = new OpfsArchiveWriter();
		await writer.append({ version: 1, session_id: 'reconnect', track_id: 'microphone', seq: 0, sample_rate: 48000, absolute_sample_start: 0, duration_samples: 480, codec: 'opus', codec_config: 'audio/webm', flags: { final: false, discontinuity: false }, idempotency_key: 'reconnect:microphone:0', payload: new Uint8Array([1, 2]) });
		const engine = new RemoteEngine({ url: 'ws://127.0.0.1:4173', reconnectMs: 20 });
		const controller = new AbortController();
		const timeout = setTimeout(() => controller.abort(), 5000);
		try {
			await engine.start('reconnect');
			await replayToRemote('reconnect', engine, controller.signal);
			return { durable: engine.durableSequence('microphone'), backlog: engine.backlog() };
		} finally { clearTimeout(timeout); await engine.cancel('reconnect'); }
	});
	expect(result).toEqual({ durable: 0, backlog: 0 });
	expect(connections).toBeGreaterThanOrEqual(2);
	expect(received).toEqual([0, 0]); // replay before ACK is expected, not duplicated archive data
});

test('metadata quota failure preserves already written audio for recovery', async ({ page }) => {
	await page.goto('/');
	const result = await page.evaluate(async () => {
		const { OpfsArchiveWriter } = await import('/src/lib/browser-recorder.ts');
		const { readOpfsArchive } = await import('/src/lib/browser-storage.ts');
		const { listBrowserMetadata } = await import('/src/lib/browser-storage.ts');
		await listBrowserMetadata(); // Complete schema upgrade before injecting write failure.
		const original = IDBObjectStore.prototype.put;
		IDBObjectStore.prototype.put = function () { throw new DOMException('Synthetic quota failure', 'QuotaExceededError'); };
		let failed = false;
		try {
			await new OpfsArchiveWriter().append({ version: 1, session_id: 'quota', track_id: 'microphone', seq: 0, sample_rate: 48000, absolute_sample_start: 0, duration_samples: 480, codec: 'opus', codec_config: 'audio/webm', flags: { final: false, discontinuity: false }, idempotency_key: 'quota:microphone:0', payload: new Uint8Array([7, 8, 9]) });
		} catch { failed = true; }
		finally { IDBObjectStore.prototype.put = original; }
		return { failed, audio: Array.from(new Uint8Array(await (await readOpfsArchive('sessions/quota/microphone.webm')).arrayBuffer())) };
	});
	expect(result).toEqual({ failed: true, audio: [7, 8, 9] });
});

test('browser file import does not require native model or Desktop dialog', async ({ page }) => {
	await page.goto('/');
	const errors: string[] = [];
	page.on('pageerror', (error) => errors.push(error.message));
	const chooser = page.waitForEvent('filechooser');
	await page.getByRole('button', { name: 'Открыть', exact: true }).click();
	await (await chooser).setFiles({ name: 'imported.wav', mimeType: 'audio/wav', buffer: Buffer.from([82, 73, 70, 70, 0, 0, 0, 0]) });
	await expect(page.getByTestId('recording-item')).toContainText('imported.wav');
	await page.reload();
	await expect(page.getByTestId('recording-item')).toContainText('imported.wav');
	expect(errors).toEqual([]);
});

// Synthetic device integration through the actual product route, not capture-harness.
test('main web application records, reloads and exports local audio without Tauri', async ({ page, context }) => {
	const pageErrors: string[] = [];
	page.on('pageerror', (error) => pageErrors.push(error.message));
	await page.addInitScript(() => {
		navigator.mediaDevices.getUserMedia = async () => {
			const context = new AudioContext();
			const oscillator = context.createOscillator();
			const destination = context.createMediaStreamDestination();
			oscillator.connect(destination);
			oscillator.start();
			await context.resume();
			return destination.stream;
		};
		navigator.mediaDevices.getDisplayMedia = async () => {
			throw new DOMException('No system audio available', 'NotAllowedError');
		};
	});
	await page.goto('/');
	await page.getByRole('button', { name: 'Записать', exact: true }).click();
	await context.setOffline(true);
	await page.getByRole('button', { name: '● Запись', exact: true }).click();
	await expect(page.getByRole('button', { name: '■ Стоп', exact: true })).toBeEnabled();
	await page.getByRole('button', { name: 'Пауза', exact: false }).click();
	await expect(page.getByRole('button', { name: 'Пауза', exact: false })).toHaveAttribute('aria-pressed', 'true');
	await page.getByRole('button', { name: 'Пауза', exact: false }).click();
	await page.waitForTimeout(1500);
	await page.getByRole('button', { name: '■ Стоп', exact: true }).click();
	await expect(page.getByTestId('recording-item').first()).toBeVisible();
	await context.setOffline(false);
	await page.reload();
	await expect(page.getByTestId('recording-item').first()).toBeVisible();
	await page.getByTestId('recording-item').first().click();
	await expect(page.locator('audio')).toHaveAttribute('src', /^blob:/);
	await page.locator('audio').evaluate(async (audio: HTMLAudioElement) => { await audio.play(); });
	await expect.poll(() => page.locator('audio').evaluate((audio: HTMLAudioElement) => audio.currentTime)).toBeGreaterThan(0);
	const download = page.waitForEvent('download');
	await page.getByRole('link', { name: 'Скачать аудио', exact: true }).click();
	expect((await download).suggestedFilename()).toMatch(/\.(ogg|webm|wav|flac|mp3)$/);
	await page.getByRole('textbox', { name: 'Название папки' }).fill('Archive');
	await page.getByRole('button', { name: 'Создать папку', exact: true }).click();
	await expect(page.getByRole('button', { name: '📁 Archive', exact: true })).toBeVisible();
	const folderId = await page.getByRole('combobox', { name: 'Переместить запись', exact: true }).locator('option').filter({ hasText: 'Archive' }).getAttribute('value');
	await page.getByRole('combobox', { name: 'Переместить запись', exact: true }).selectOption(folderId!);
	await expect(page.getByTestId('recording-item')).toHaveCount(0);
	await page.getByRole('button', { name: '📁 Archive', exact: true }).click();
	await expect(page.getByTestId('recording-item')).toHaveCount(1);
	await page.reload();
	await page.getByRole('button', { name: '📁 Archive', exact: true }).click();
	await expect(page.getByTestId('recording-item')).toHaveCount(1);
	await page.getByRole('textbox', { name: 'Название папки' }).fill('Nested');
	await page.getByRole('button', { name: 'Создать папку', exact: true }).click();
	await page.getByTestId('recording-item').dragTo(page.getByRole('button', { name: '📁 Nested', exact: true }));
	await expect(page.getByTestId('recording-item')).toHaveCount(0);
	await page.getByRole('button', { name: '📁 Nested', exact: true }).click();
	await expect(page.getByTestId('recording-item')).toHaveCount(1);
	await page.getByRole('button', { name: '← Nested', exact: true }).click();
	page.once('dialog', (dialog) => dialog.accept('Renamed'));
	await page.getByRole('button', { name: 'Переименовать папку', exact: true }).click();
	await expect(page.getByRole('button', { name: '📁 Renamed', exact: true })).toBeVisible();
	page.once('dialog', (dialog) => dialog.accept());
	await page.getByRole('button', { name: 'Удалить папку', exact: true }).click();
	await expect(page.getByTestId('recording-item')).toHaveCount(1);
	const integrity = await page.evaluate(async () => {
		const { listBrowserMetadata, readOpfsArchive } = await import('/src/lib/browser-storage.ts');
		const archives = (await listBrowserMetadata()).filter((entry) => String(entry.key).startsWith('archive:'));
		return Promise.all(archives.map(async (entry) => ({ bytes: (await readOpfsArchive(String(entry.path))).size, hash: entry.content_hash, blocks: entry.block_count })));
	});
	expect(integrity.length).toBeGreaterThan(0);
	for (const archive of integrity) {
		expect(archive.bytes).toBeGreaterThan(0);
		expect(archive.hash).toMatch(/^[a-f0-9]{64}$/);
		expect(Number(archive.blocks)).toBeGreaterThan(0);
	}
	const encoding = await page.evaluate(async () => {
		const { listBrowserMetadata, readOpfsArchive } = await import('/src/lib/browser-storage.ts');
		const entry = (await listBrowserMetadata()).find((row) => String(row.key).startsWith('archive:'))!;
		const file = await readOpfsArchive(String(entry.path));
		return { path: String(entry.path), mime: String(entry.mime_type), header: Array.from(new Uint8Array(await file.slice(0, 4).arrayBuffer())) };
	});
	if (encoding.mime.includes('ogg')) {
		expect(encoding.path).toMatch(/\.ogg$/);
		expect(encoding.header).toEqual([79, 103, 103, 83]);
	} else {
		expect(encoding.path).toMatch(/\.webm$/);
		expect(encoding.header).toEqual([26, 69, 223, 163]);
	}
	await page.getByRole('button', { name: 'Переименовать запись', exact: true }).click();
	const rename = page.locator('input[data-place="list"]');
	await rename.fill('Renamed recording');
	await rename.press('Enter');
	await expect(page.getByTestId('recording-item')).toContainText('Renamed recording');
	await page.reload();
	await page.getByRole('button', { name: '📁 Archive', exact: true }).click();
	await expect(page.getByTestId('recording-item')).toContainText('Renamed recording');
	expect(pageErrors).toEqual([]);
});
