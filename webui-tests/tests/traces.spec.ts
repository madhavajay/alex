import { test, expect, openUi } from './fixtures';

test('traces render, filter, open metadata, and paginate at 25 rows', async ({ page, runtime }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openUi(page, runtime);
  await page.locator('nav [data-view="traces"]').click();

  const rows = page.locator('#trace-list .trace-session-row');
  await expect(rows).toHaveCount(25);
  await expect(page.locator('#more-traces')).toBeVisible();

  await page.locator('#more-traces').click();
  await expect(rows).toHaveCount(28);

  await page.locator('#trace-filters select[name="provider"]').selectOption('openai');
  await page.getByRole('button', { name: 'Apply', exact: true }).click();
  await expect(rows).toHaveCount(14);
  await expect(rows).toContainText(Array(14).fill('OpenAI'));

  await rows.first().click();
  const detail = page.locator('#trace-detail');
  await expect(detail.locator('.trace-detail-head')).toBeVisible();
  await expect(detail).toContainText('Provenance');
  await expect(detail.locator('.trace-detail-grid div').filter({ hasText: 'Harness' })).toContainText(/webui-playwright-openai/i);
  await expect(detail).toContainText(/webui-seed-\d+/);

  await page.getByRole('button', { name: 'Clear all' }).click();
  await expect(rows).toHaveCount(25);
  await expect(page.locator('#more-traces')).toBeVisible();
});
