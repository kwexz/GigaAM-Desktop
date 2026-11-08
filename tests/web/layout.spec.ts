import { expect, test } from '@playwright/test';

for (const viewport of [{ width: 1920, height: 1080 }, { width: 2560, height: 1440 }]) {
	test(`web layout stays within viewport ${viewport.width}`, async ({ page }) => {
		await page.setViewportSize(viewport);
		await page.goto('/');
		const bounds = await page.locator('.app-shell').boundingBox();
		expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewport.height + 1);
		const header = await page.locator('header').boundingBox();
		expect(header!.height).toBeLessThan(100);
		expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
	});
}
