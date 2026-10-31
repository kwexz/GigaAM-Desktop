import { expect, test } from '@playwright/test';

test('folders start collapsed and transcript clear preserves recording', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(async () => {
		const s = await import('/src/lib/browser-storage.ts');
		await s.putBrowserRecording({ id: 'actions', name: 'Actions', created_ms: 1, duration_seconds: 1 });
		await s.saveBrowserOrganization([{ id: 'folder', kind: 'folder', name: 'Folder', parent_id: null, position: 0 }, { id: 'actions', kind: 'recording', name: 'Actions', parent_id: 'folder', position: 0 }]);
		await s.putSegment({ version: 1, session_id: 'actions', segment_id: 'segment', track_id: 'file', seq: 0, start_seconds: 0, end_seconds: 1, text: 'Text to clear', final: true, revision: 1, idempotency_key: 'actions:segment' });
	});
	await page.reload();
	await expect(page.getByTestId('recording-item')).toHaveCount(0);
	await page.getByRole('button', { name: 'Expand folder' }).click();
	await page.getByTestId('recording-item').click();
	await expect(page.locator('footer .tools')).toBeVisible();
	await page.locator('footer').getByRole('button', { name: 'Очистить', exact: true }).click();
	await page.getByRole('dialog').getByRole('button', { name: 'Очистить', exact: true }).click();
	await expect(page.getByRole('status').filter({ hasText: 'Текст очищен' })).toBeVisible();
	await expect(page.getByTestId('recording-item')).toContainText('Actions');
	await expect(page.getByText('Text to clear', { exact: true })).toHaveCount(0);
});
