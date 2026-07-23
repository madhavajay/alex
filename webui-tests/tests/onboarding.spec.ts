import { test, expect, openUi } from './fixtures';

test('dashboard and providers render seeded account health and stats', async ({ page, request, runtime }) => {
  const response = await request.get(`${runtime.baseUrl}/admin/accounts`, { headers: { 'x-api-key': runtime.localKey } });
  expect(response.ok()).toBe(true);
  const payload = await response.json() as {
    accounts: Array<{ id: string; provider: string; name: string; health?: string; status?: string }>;
  };

  await openUi(page, runtime);

  const dashboardAccounts = page.locator('#dashboard-accounts .account-row');
  await expect(dashboardAccounts).toHaveCount(4);
  for (const provider of ['anthropic', 'openai', 'openrouter', 'amp']) {
    const account = payload.accounts.find(value => value.provider === provider)!;
    const row = dashboardAccounts.filter({ hasText: provider });
    const health = account.health === 'unknown' ? 'not checked yet' : account.health?.replaceAll('_', ' ');
    await expect(row).toContainText(account.name);
    await expect(row).toContainText(health || account.status || 'not checked yet');
  }

  const stats = page.locator('#dashboard-stats .stat-card');
  await expect(stats).toHaveCount(4);
  await expect(stats).toContainText(['Last hour', 'Last 24 hours', '24h cost', 'In flight']);
  await expect(page.locator('#dashboard-limits [data-credit-provider="openrouter"]')).toContainText('💰 $30.25 credits');
  await expect(page.locator('#dashboard-limits [data-credit-provider="amp"]')).toContainText('💰 $5.00 credits');

  await page.locator('nav [data-view="providers"]').click();
  const providerAccounts = page.locator('#provider-accounts [data-account-card]');
  await expect(providerAccounts).toHaveCount(4);
  for (const account of payload.accounts) {
    const card = page.locator(`[data-account-card="${account.id}"]`);
    const health = account.health === 'unknown' ? 'not checked yet' : account.health?.replaceAll('_', ' ');
    await expect(card).toContainText(account.provider);
    await expect(card).toContainText(account.name);
    await expect(card).toContainText(health || account.status || 'not checked yet');
    await expect(card).toContainText(account.status || 'active');
  }
  await expect(page.locator('[data-account-card="mock-openrouter"] [data-credit-balance]')).toContainText('💰 $30.25 credits');
  await expect(page.locator('[data-account-card="mock-amp"] [data-credit-balance]')).toContainText('💰 $5.00 credits');
});
