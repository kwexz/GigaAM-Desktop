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
