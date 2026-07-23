import { test, expect, openUi } from './fixtures';

test('unfinished onboarding locks same-tab navigation but allows a direct trace tab', async ({ page, request, runtime }) => {
  const setCompleted = async (completed: boolean) => {
    const response = await request.post(`${runtime.baseUrl}/admin/web/onboarding`, {
      headers: { 'x-api-key': runtime.localKey },
      data: { completed }
    });
    expect(response.ok()).toBe(true);
  };

  await setCompleted(false);
  try {
    await page.goto(`${runtime.baseUrl}/ui`);
    await expect(page.locator('#onboarding-view')).toBeVisible();
    await expect(page.locator('.sidebar nav')).toBeHidden();

    await page.evaluate(() => { location.hash = '#providers/codex'; });
    await expect(page).toHaveURL(/#onboarding$/);
    await expect(page.locator('#onboarding-view')).toBeVisible();

    const traceTab = await page.context().newPage();
    try {
      await traceTab.goto(`${runtime.baseUrl}/ui#traces`);
      await expect(traceTab.locator('#traces-view')).toBeVisible();
      await expect(traceTab).toHaveURL(/\/ui#traces$/);
    } finally {
      await traceTab.close();
    }
  } finally {
    await setCompleted(true);
  }
});

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

  await page.evaluate(() => { location.hash = '#providers'; });
  await expect(page).toHaveURL(/#providers\/claude$/);
  await expect(page.locator('#provider-tabs [data-section-tab=""]')).toHaveCount(0);
  const providerAccounts = page.locator('#provider-accounts [data-account-card]');
  await expect(providerAccounts).toHaveCount(1);
  const providerTabs: Record<string, string> = { anthropic: 'claude', openai: 'codex', openrouter: 'openrouter', amp: 'amp' };
  for (const account of payload.accounts) {
    const tab = providerTabs[account.provider];
    await page.locator(`#provider-tabs [data-section-tab="${tab}"]`).click();
    await expect(page).toHaveURL(new RegExp(`#providers/${tab}$`));
    await expect(providerAccounts).toHaveCount(1);
    const card = page.locator(`[data-account-card="${account.id}"]`);
    const health = account.health === 'unknown' ? 'not checked yet' : account.health?.replaceAll('_', ' ');
    await expect(card).toContainText(account.provider);
    await expect(card).toContainText(account.name);
    await expect(card).toContainText(health || account.status || 'not checked yet');
    await expect(card).toContainText(account.status || 'active');
    if (account.provider === 'openrouter') await expect(card.locator('[data-credit-balance]')).toContainText('💰 $30.25 credits');
    if (account.provider === 'amp') await expect(card.locator('[data-credit-balance]')).toContainText('💰 $5.00 credits');
  }
});
