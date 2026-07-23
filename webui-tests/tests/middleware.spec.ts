import type { Request } from '@playwright/test';
import { test, expect, openUi } from './fixtures';

const ruleId = 'alex.fable-5-to-gpt-5.6-sol';

test('middleware renders readable rules, persists toggles, and dry-runs a fixture', async ({ page, runtime }) => {
  await openUi(page, runtime);
  await page.locator('nav [data-view="middleware"]').click();

  const card = page.locator(`[data-rule-card="${ruleId}"]`);
  await expect(card).toBeVisible();
  await expect(card).toContainText('Fable 5 → GPT-5.6 Sol');
  await expect(card.locator('p.muted')).toContainText('model regex: ^claude-fable-5$');
  await expect(card.locator('p.muted')).toContainText('reroute to gpt-5.6-sol');

  const toggle = card.locator(`[data-rule-toggle="${ruleId}"]`);
  const initial = await toggle.getAttribute('aria-pressed');
  let resolvePut!: (request: Request) => void;
  const putRequest = new Promise<Request>(resolve => { resolvePut = resolve; });
  await page.route(`**/admin/middleware/rules/${ruleId}`, async route => {
    if (route.request().method() === 'PUT') resolvePut(route.request());
    await route.continue();
  });

  await toggle.click();
  const request = await putRequest;
  expect(request.method()).toBe('PUT');
  expect(request.postDataJSON().enabled).toBe(initial !== 'true');
  await expect(toggle).toHaveAttribute('aria-pressed', initial === 'true' ? 'false' : 'true');

  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', initial!);

  await card.locator('select[name="fixture"]').selectOption('anthropic-fable-refusal-200');
  await card.getByRole('button', { name: 'Run dry test' }).click();
  const result = card.locator(`[data-rule-result="${ruleId}"]`);
  await expect(result).toContainText('Decision: reroute');
  await expect(result).toContainText(`Matched: ${ruleId}`);
});
