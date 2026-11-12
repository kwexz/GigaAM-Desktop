import { expect, test } from '@playwright/test';

test('tree expands, collapses, renames on double click and moves records by drag', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(async () => {
		const s = await import('/src/lib/browser-storage.ts');
		await s.putBrowserRecording({ id: 'tree-audio', name: 'Audio', created_ms: 1, duration_seconds: 1 });
		await s.saveBrowserOrganization([
			{ id: 'tree-folder', kind: 'folder', parent_id: null, name: 'Folder', position: 0 },
			{ id: 'tree-audio', kind: 'recording', parent_id: 'tree-folder', name: 'Audio', position: 0 }
		]);
	});
	await page.reload();
	await page.getByRole('button', { name: 'Expand folder' }).click();
	await expect(page.getByTestId('recording-item')).toBeVisible();
	await page.getByRole('button', { name: 'Collapse folder' }).click();
	await expect(page.getByTestId('recording-item')).toHaveCount(0);
	await page.getByRole('button', { name: 'Expand folder' }).click();
	await page.getByRole('button', { name: '📁 Folder', exact: true }).dblclick();
	await page.getByRole('textbox', { name: 'Folder name' }).fill('Renamed');
	await page.getByRole('textbox', { name: 'Folder name' }).press('Enter');
	await expect(page.getByRole('button', { name: '📁 Renamed', exact: true })).toBeVisible();
	await page.getByTestId('recording-item').dragTo(page.getByRole('tree'));
	await expect.poll(() => page.evaluate(async () => {
		const { loadBrowserOrganization } = await import('/src/lib/browser-storage.ts');
		return (await loadBrowserOrganization()).find((node) => node.id === 'tree-audio')?.parent_id;
	})).toBeNull();
	const heading = await page.locator('.search-row').boundingBox();
	const add = await page.getByRole('button', { name: 'Создать папку' }).boundingBox();
	expect(add!.y).toBeGreaterThanOrEqual(heading!.y);
	expect(add!.y + add!.height).toBeLessThanOrEqual(heading!.y + heading!.height + 1);
	const trash = page.getByRole('button', { name: 'Удалить папку' });
	await page.getByRole('button', { name: '📁 Renamed', exact: true }).hover();
	await expect(trash).toHaveCSS('opacity', '1');
	await trash.click();
	await page.getByRole('dialog').getByRole('button', { name: 'Удалить', exact: true }).click();
	await expect(page.getByRole('button', { name: '📁 Renamed', exact: true })).toHaveCount(0);
	await expect(page.getByTestId('recording-item')).toBeVisible();
});

test('pointer drag moves folders and recordings into a folder without native HTML drag', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(async () => {
		const s = await import('/src/lib/browser-storage.ts');
		await s.putBrowserRecording({ id: 'pointer-audio', name: 'Pointer audio', created_ms: 1, duration_seconds: 1 });
		await s.saveBrowserOrganization([
			{ id: 'pointer-target', kind: 'folder', parent_id: null, name: 'Target', position: 0 },
			{ id: 'pointer-folder', kind: 'folder', parent_id: null, name: 'Source folder', position: 1 },
			{ id: 'pointer-audio', kind: 'recording', parent_id: null, name: 'Pointer audio', position: 2 }
		]);
	});
	await page.reload();
	const move = async (id: string) => {
		const source = await page.locator(`[data-tree-id="${id}"]`).boundingBox();
		const target = await page.locator('[data-tree-id="pointer-target"]').boundingBox();
		await page.mouse.move(source!.x + source!.width / 2, source!.y + source!.height / 2);
		await page.mouse.down();
		await page.mouse.move(target!.x + target!.width / 2, target!.y + target!.height / 2, { steps: 8 });
		await page.mouse.up();
		await expect.poll(() => page.evaluate(async (nodeId) => {
			const { loadBrowserOrganization } = await import('/src/lib/browser-storage.ts');
			return (await loadBrowserOrganization()).find((node) => node.id === nodeId)?.parent_id;
		}, id)).toBe('pointer-target');
	};
	await move('pointer-folder');
	await move('pointer-audio');
});
