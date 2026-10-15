import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

const OUT = process.env.SHOTS_DIR ?? 'test-results/shots';
mkdirSync(OUT, { recursive: true });

const SIZES = [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'mobile', width: 390, height: 844 },
] as const;
const THEMES = ['dark', 'light'] as const;

async function shot(page: Page, name: string) {
  await page.waitForTimeout(350);
  await page.screenshot({ path: join(OUT, `${name}.png`) });
}

/** Deselect everything (click empty canvas) and fit the whole graph. */
async function fitAll(page: Page) {
  await page.getByRole('button', { name: 'Fit view (F)' }).click();
  await page.waitForTimeout(400);
  // After fitting, the canvas corner is empty: clicking it clears the selection.
  await page.locator('.react-flow__pane').click({ position: { x: 6, y: 6 } });
  await page.waitForTimeout(150);
}

/** Select one node (click its kind label) and fit it at up to 100 %. */
async function fitNode(page: Page, testId: string) {
  await page.getByTestId(testId).locator('.node-kind').click();
  await page.keyboard.press('f');
  await page.waitForTimeout(450);
}

async function pickFromLibrary(page: Page, nodeIndex: number, itemIndex: number) {
  const node = page.getByTestId('node-reference').nth(nodeIndex);
  await node.getByRole('button', { name: 'Library' }).click();
  const item = node.locator('.picker-item').nth(itemIndex);
  await item.waitFor();
  await item.click();
  await expect(node.locator('.ref-image img')).toBeVisible();
}

for (const theme of THEMES) {
  for (const size of SIZES) {
    test(`${size.name} ${theme}`, async ({ page }) => {
      const tag = `${size.name}-${theme}`;
      await page.setViewportSize({ width: size.width, height: size.height });
      await page.addInitScript((t) => {
        try {
          localStorage.setItem('ps.theme', t);
        } catch {
          /* ignore */
        }
      }, theme);
      await page.goto('/?mock=fast');
      await page.locator('.empty-card').waitFor();
      await shot(page, `01-empty-${tag}`);

      await page.getByRole('button', { name: 'Editorial edit from references' }).first().click();
      await page.getByTestId('node-edit').waitFor();
      await pickFromLibrary(page, 0, 8);
      await pickFromLibrary(page, 1, 7);
      await fitAll(page);
      await expect(page.getByTestId('node-edit').locator('.cost-main')).toContainText('cr');
      await shot(page, `02-editorial-template-${tag}`);

      // Generate-type node with its schema form and live cost.
      const edit = page.getByTestId('node-edit');
      if (size.name === 'desktop') {
        await fitNode(page, 'node-edit');
        await edit.screenshot({ path: join(OUT, `03-edit-node-form-${tag}.png`) });
        await fitAll(page);
      }

      await edit.getByRole('button', { name: 'Run', exact: true }).click();
      await expect(edit.locator('.result-skeleton').first()).toBeVisible();
      await shot(page, `04-running-${tag}`);
      await expect(edit.locator('.results-wrap .result')).toHaveCount(8, { timeout: 60_000 });
      await page.waitForTimeout(600);
      await fitAll(page);
      await shot(page, `05-results-${tag}`);
      if (size.name === 'desktop') {
        await fitNode(page, 'node-edit');
        await edit.screenshot({ path: join(OUT, `06-results-node-${tag}.png`) });
        await fitAll(page);
      }

      const review = page.getByTestId('node-review');
      await review.getByRole('button', { name: 'Review', exact: true }).click();
      await expect(review.locator('.scores')).toBeVisible({ timeout: 60_000 });
      await expect(review.locator('.review-thumb')).toHaveCount(8, { timeout: 60_000 });
      if (size.name === 'desktop') {
        await fitNode(page, 'node-review');
        await review.screenshot({ path: join(OUT, `07-review-node-${tag}.png`) });
        await fitAll(page);
      }

      await edit.locator('.result-open').first().click();
      await page.locator('.viewer').waitFor();
      await page.waitForFunction(() => {
        const img = document.querySelector<HTMLImageElement>('.viewer-img:not(.is-thumb)');
        return Boolean(img && img.complete && img.naturalWidth > 0);
      });
      await shot(page, `08-viewer-${tag}`);
      await page.keyboard.press('Escape');

      await page.getByRole('tab', { name: 'App' }).click();
      await page.locator('.appmode').waitFor();
      await shot(page, `09-app-mode-${tag}`);
      await page.getByRole('tab', { name: 'Canvas' }).click();

      if (size.name === 'desktop') {
        // A plain Generate node, its model wired from a Director plan.
        await page.getByRole('button', { name: /Portrait \/ headshot/ }).click();
        await page.getByTestId('node-director').waitFor();
        await pickFromLibrary(page, 0, 8);
        await page.getByTestId('node-director').getByRole('button', { name: 'Direct the shot' }).click();
        await expect(page.getByTestId('node-director').locator('.plan li')).toHaveCount(2, { timeout: 30_000 });
        const gen = page.getByTestId('node-generate');
        await expect(gen.locator('.cost-main')).toContainText('cr');
        await fitAll(page);
        await shot(page, `10-director-generate-${tag}`);
        await fitNode(page, 'node-generate');
        await gen.screenshot({ path: join(OUT, `11-generate-node-${tag}.png`) });
        await gen.locator('.model-button').click();
        await page.waitForTimeout(200);
        await gen.screenshot({ path: join(OUT, `12-model-picker-${tag}.png`) });
        await page.keyboard.press('Escape');
      }
    });
  }
}

test('login and error states', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/?mock=loggedout');
  await page.locator('.login-card').waitFor();
  await shot(page, '13-login-desktop-dark');
  await page.setViewportSize({ width: 390, height: 844 });
  await shot(page, '13-login-mobile-dark');
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/?mock=noopenart,noclaude');
  await page.locator('.empty-card').waitFor();
  await page.getByRole('button', { name: /Portrait \/ headshot/ }).first().click().catch(async () => {
    await page.getByRole('button', { name: 'Nodes and templates' }).click();
    await page.getByRole('button', { name: /Portrait \/ headshot/ }).click();
  });
  await page.getByTestId('node-generate').locator('.error-note').waitFor();
  await page.locator('.lib-grid').waitFor();
  await shot(page, '14-not-connected-desktop-dark');
  await fitNode(page, 'node-generate');
  await page.getByTestId('node-generate').screenshot({ path: join(OUT, '15-not-connected-node-desktop-dark.png') });
});
