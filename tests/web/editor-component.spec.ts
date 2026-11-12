import { expect, test } from '@playwright/test';

test('markdown editor switches modes without overlay and retains text', async ({ page }) => {
	await page.setViewportSize({ width: 1010, height: 640 });
	await page.goto('/');
	await page.evaluate(async () => {
		const { mountEditor } = await import('/src/lib/editor-harness.ts');
		const target = document.createElement('div'); target.id = 'editor-test'; target.style.cssText = 'position:fixed;inset:40px 40px 100px;display:flex;flex-direction:column'; document.body.append(target);
		const actions = document.createElement('button'); actions.id = 'editor-actions-test'; actions.textContent = 'Save'; actions.style.cssText = 'position:fixed;bottom:30px'; document.body.append(actions);
		mountEditor(target);
	});
	const editor = page.locator('#editor-test');
	await expect(editor.locator('.toastui-editor-ww-container')).toBeVisible();
	await expect(editor.locator('.toastui-editor-md-container')).toBeHidden();
	const icon = editor.locator('.toastui-editor-toolbar-icons.bold');
	await expect(icon).toBeVisible();
	await expect.poll(() => icon.evaluate((element) => getComputedStyle(element).backgroundImage)).toContain('data:image/');
	await editor.locator('.toastui-editor-mode-switch .tab-item').first().click();
	await expect(editor.locator('.toastui-editor-md-container')).toBeVisible();
	await expect(editor.locator('.toastui-editor-ww-container')).toBeHidden();
	await expect(editor.locator('.toastui-editor-md-container')).toContainText('**Bold text**');
	await editor.locator('.toastui-editor-mode-switch .tab-item').last().click();
	await expect(editor.locator('.toastui-editor-ww-container')).toBeVisible();
	await expect(editor.locator('.toastui-editor-ww-container')).toContainText('Bold text');
	const box = await editor.boundingBox(); const actions = await page.locator('#editor-actions-test').boundingBox();
	expect(box!.y + box!.height).toBeLessThanOrEqual(actions!.y);
});
