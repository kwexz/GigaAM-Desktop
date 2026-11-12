import { expect, test } from '@playwright/test';

test('long microphone name stays within the sidebar and is ellipsized', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(async () => {
		const { mountDropdown } = await import('/src/lib/editor-harness.ts');
		const target = document.createElement('div'); target.id = 'dropdown-test'; target.style.width = '180px'; document.body.append(target);
		mountDropdown(target);
	});
	const summary = page.locator('#dropdown-test summary');
	const label = summary.locator('.selected-label');
	await expect(summary).toBeVisible();
	expect(await summary.evaluate((node) => node.getBoundingClientRect().width)).toBeLessThanOrEqual(180);
	expect(await label.evaluate((node) => ({ overflow: getComputedStyle(node).textOverflow, clipped: node.scrollWidth > node.clientWidth }))).toEqual({ overflow: 'ellipsis', clipped: true });
});
