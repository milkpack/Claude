import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

const OUT = process.env.SHOTS_DIR ?? 'test-results/shots';
mkdirSync(OUT, { recursive: true });

async function fitAll(page: Page) {
  await page.getByRole('button', { name: 'Fit view (F)' }).click();
  await page.waitForTimeout(400);
  await page.locator('.react-flow__pane').click({ position: { x: 6, y: 6 } });
  await page.waitForTimeout(150);
}
async function fitNode(page: Page, testId: string, nth = 0) {
  await page.getByTestId(testId).nth(nth).locator('.node-kind').click();
  await page.keyboard.press('f');
  await page.waitForTimeout(450);
}
async function pick(page: Page, nodeIndex: number, itemIndex: number) {
  const node = page.getByTestId('node-reference').nth(nodeIndex);
  await node.getByRole('button', { name: 'Library' }).click();
  await node.locator('.picker-item').nth(itemIndex).click();
  await expect(node.locator('.ref-image img')).toBeVisible();
}
async function drag(page: Page, box: { x: number; y: number; width: number; height: number }, pts: [number, number][]) {
  const at = ([x, y]: [number, number]) => [box.x + x * box.width, box.y + y * box.height] as const;
  const [sx, sy] = at(pts[0] ?? [0, 0]);
  await page.mouse.move(sx, sy);
  await page.mouse.down();
  for (const p of pts.slice(1)) {
    const [x, y] = at(p);
    await page.mouse.move(x, y, { steps: 6 });
  }
  await page.mouse.up();
}

for (const theme of ['dark', 'light'] as const) {
  test(`handoff ${theme}`, async ({ page }) => {
    test.setTimeout(420_000);
    const tag = `desktop-${theme}`;
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.addInitScript((t) => localStorage.setItem('ps.theme', t), theme);
    await page.goto('/?mock=fast');
    await page.locator('.empty-card').waitFor();
    // Toasts would cover node close-ups.
    await page.addStyleTag({ content: '.toasts{display:none !important}' });

    // Session 1: the editorial template with references (gives it a cover).
    await page.getByRole('button', { name: 'Editorial edit from references' }).first().click();
    await pick(page, 0, 8);
    await pick(page, 1, 7);
    await expect(page.locator('.save-state')).toHaveText('Saved', { timeout: 10_000 });
    expect(await page.evaluate(() => window.location.hash)).toMatch(/^#s-/);

    // Session 2: portrait template → Director handoff.
    await page.getByRole('button', { name: /Portrait \/ headshot/ }).click();
    await page.getByTestId('node-director').waitFor();
    await page.waitForTimeout(300);
    await pick(page, 0, 4);
    const director = page.getByTestId('node-director');
    await fitNode(page, 'node-director');
    await director.getByRole('button', { name: 'Create task for Claude' }).click();
    await expect(director.getByTestId('task-card')).toContainText('waiting');
    await director.getByRole('button', { name: /Paste answer/ }).click();
    await director.screenshot({ path: join(OUT, `01-director-handoff-card-${tag}.png`) });
    await expect(director.locator('.plan li').first()).toBeVisible({ timeout: 20_000 });
    await page.waitForTimeout(300);
    await director.screenshot({ path: join(OUT, `02-director-handoff-answered-${tag}.png`) });

    // Session 3: Fix with Photoshop.
    await page.getByRole('button', { name: /Fix with Photoshop/ }).click();
    await page.getByTestId('node-sketch').waitFor();
    await page.waitForTimeout(300);
    await pick(page, 0, 8);
    await fitAll(page);
    await page.screenshot({ path: join(OUT, `03-fix-with-photoshop-${tag}.png`) });

    // Sketch: pen, circle, arrow, text, then save.
    await fitNode(page, 'node-sketch');
    const sketch = page.getByTestId('node-sketch');
    const svg = sketch.locator('.sketch-svg');
    let box = await svg.boundingBox();
    if (!box) throw new Error('no sketch surface');
    await sketch.getByRole('button', { name: 'Circle' }).click();
    await drag(page, box, [[0.36, 0.22], [0.58, 0.4]]);
    await sketch.getByRole('button', { name: 'Arrow' }).click();
    await drag(page, box, [[0.82, 0.12], [0.6, 0.27]]);
    await sketch.getByRole('button', { name: 'Colour #f5c542' }).click();
    await sketch.getByRole('button', { name: 'Pen' }).click();
    await drag(page, box, [[0.2, 0.75], [0.3, 0.7], [0.42, 0.78], [0.55, 0.72], [0.66, 0.8]]);
    await sketch.getByRole('button', { name: 'Text' }).click();
    await sketch.getByLabel('Text to place').fill('1 hair');
    box = await svg.boundingBox();
    if (!box) throw new Error('no sketch surface');
    await page.mouse.click(box.x + box.width * 0.62, box.y + box.height * 0.42);
    await sketch.getByLabel('Notes').fill('1 — stray hair over the left eye; 2 — collar fold');
    await sketch.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(sketch.getByRole('button', { name: 'Saved', exact: true })).toBeVisible({ timeout: 10_000 });
    await sketch.screenshot({ path: join(OUT, `04-sketch-node-${tag}.png`) });

    // Photoshop: paint a mask, create the task, receive the result.
    await fitNode(page, 'node-photoshop');
    const ps = page.getByTestId('node-photoshop');
    const surf = await ps.locator('.mask-surface').boundingBox();
    if (!surf) throw new Error('no mask surface');
    await drag(page, surf, [[0.4, 0.26], [0.48, 0.3], [0.56, 0.27], [0.5, 0.34], [0.42, 0.33]]);
    await expect(ps.locator('.hint').first()).toContainText('mask set');
    await ps.screenshot({ path: join(OUT, `05-photoshop-mask-${tag}.png`) });
    await ps.getByRole('button', { name: 'Create Photoshop task' }).click();
    await expect(ps.getByTestId('task-card')).toContainText('waiting');
    await expect(ps.locator('.results .result')).toHaveCount(1, { timeout: 20_000 });
    await fitNode(page, 'node-photoshop');
    await ps.screenshot({ path: join(OUT, `06-photoshop-result-${tag}.png`) });

    // Blender node from the palette.
    await page.getByRole('button', { name: /^Blender/ }).click();
    const blender = page.getByTestId('node-blender');
    await fitNode(page, 'node-blender');
    await blender.getByLabel('Subject').fill('The amber glass bottle, 1:1 scale, label facing camera');
    await blender.locator('input').nth(0).fill('wet slate plinth, dark seamless');
    await blender.locator('input').nth(1).fill('eye level, slight high angle');
    await blender.locator('input').nth(2).fill('100 mm');
    await blender.locator('input').nth(3).fill('raking backlight, soft fill left');
    await blender.getByRole('button', { name: 'side' }).click();
    await blender.getByRole('button', { name: 'Create Blender task' }).click();
    await expect(blender.locator('.results .result')).toHaveCount(2, { timeout: 20_000 });
    await fitNode(page, 'node-blender');
    await blender.screenshot({ path: join(OUT, `07-blender-node-${tag}.png`) });

    // Session switcher with covers, and the in-page delete confirm.
    await page.getByRole('button', { name: 'Sessions' }).click();
    await expect(page.locator('.session-row')).toHaveCount(3, { timeout: 10_000 });
    await page.waitForTimeout(500);
    await page.screenshot({ path: join(OUT, `08-session-switcher-${tag}.png`) });
    await page.locator('.session-row').nth(2).getByRole('button', { name: /^Delete / }).click();
    await page.screenshot({ path: join(OUT, `09-session-delete-confirm-${tag}.png`) });
  });
}

test('mobile session switcher', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/?mock=fast');
  await page.locator('.empty-card').waitFor();
  await page.getByRole('button', { name: 'Editorial edit from references' }).first().click();
  await pick(page, 0, 8);
  await expect(page.locator('.save-state')).toHaveCount(1);
  await page.waitForTimeout(1500);
  await page.getByRole('button', { name: 'Sessions' }).click();
  await expect(page.locator('.session-row')).toHaveCount(1, { timeout: 10_000 });
  await page.waitForTimeout(400);
  await page.screenshot({ path: join(OUT, '10-session-switcher-mobile-dark.png') });
});

test('cloud routine', async ({ page }) => {
  test.setTimeout(300_000);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/?mock=fast,routine,manual');
  await page.locator('.empty-card').waitFor();
  await page.addStyleTag({ content: '.toasts{display:none !important}' });
  await page.getByRole('button', { name: 'Nodes and templates' }).isVisible();
  await page.getByRole('button', { name: /Portrait \/ headshot/ }).click();
  await page.getByTestId('node-director').waitFor();
  await page.waitForTimeout(300);
  await pick(page, 0, 4);
  await fitNode(page, 'node-director');
  const director = page.getByTestId('node-director');
  await director.getByRole('button', { name: 'Run in the cloud' }).click();
  await expect(director.locator('.task-run')).toContainText('working in the cloud');
  await director.getByRole('button', { name: 'Run again in the cloud' }).click();
  await director.screenshot({ path: join(OUT, '11-director-cloud-run-desktop-dark.png') });
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.locator('.routine-form').scrollIntoViewIfNeeded();
  await page.waitForTimeout(300);
  await page.screenshot({ path: join(OUT, '12-settings-routine-desktop-dark.png') });
});
