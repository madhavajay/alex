import { test, expect, openUi } from './fixtures';

test('traces render, filter, open metadata, and paginate at 25 rows', async ({ page, runtime }) => {
  await openUi(page, runtime);
  await page.getByRole('button', { name: 'Traces' }).click();

  const rows = page.locator('#trace-list .trace-row');
  await expect(rows).toHaveCount(25);
  await expect(page.locator('#more-traces')).toBeVisible();

  await page.getByLabel('Provider').fill('openai');
  await page.getByRole('button', { name: 'Apply filters' }).click();
  await expect(rows).toHaveCount(14);
  await expect(rows).toContainText(Array(14).fill('openai'));

  await rows.first().click();
  const detail = page.locator('#trace-detail');
  await expect(detail).toBeVisible();
  await expect(detail.getByRole('heading', { name: /^Trace / })).toBeVisible();
  await expect(detail).toContainText('Provenance');
  await expect(detail).toContainText('webui-playwright-openai');
  await expect(detail).toContainText(/webui-seed-\d+/);

  await page.getByRole('button', { name: 'Clear' }).click();
  await expect(rows).toHaveCount(25);
  await page.locator('#more-traces').click();
  await expect(rows).toHaveCount(28);
});
