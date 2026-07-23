import { test, expect, openUi } from './fixtures';

test('onboarding renders daemon status, seeded accounts, and connected provider tiles', async ({ page, request, runtime }) => {
  const response = await request.get(`${runtime.baseUrl}/admin/accounts`, { headers: { 'x-api-key': runtime.localKey } });
  expect(response.ok()).toBe(true);
  const payload = await response.json() as { accounts: Array<{ provider: string; health?: string; status?: string }> };

  await openUi(page, runtime);
  await page.getByRole('button', { name: 'Onboarding' }).click();

  const accounts = page.locator('#accounts .card');
  await expect(accounts).toHaveCount(2);
  for (const provider of ['anthropic', 'openai']) {
    const account = payload.accounts.find(value => value.provider === provider)!;
    const card = accounts.filter({ hasText: provider });
    await expect(card).toContainText('mock');
    await expect(card).toContainText(account.health || account.status || 'configured');
  }

  await expect(page.locator('[data-provider="claude"]')).toContainText('1 connected');
  await expect(page.locator('[data-provider="codex"]')).toContainText('1 connected');

  await page.getByRole('button', { name: 'Status' }).click();
  await expect(page.locator('#status-cards')).toContainText('Accounts');
  await expect(page.locator('#status-cards')).toContainText('2');
});
