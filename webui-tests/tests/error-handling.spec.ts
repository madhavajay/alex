import { test, expect, openUi } from './fixtures';

test('onboarding shows an upstream-backed admin error instead of a blank state', async ({ page, runtime }) => {
  const queued = await fetch(`${runtime.fakeBaseUrl}/_control/queue`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-control-key': runtime.fakeControlKey },
    body: JSON.stringify({ endpoint: 'GET /cliproxyapi/v1/models', response: { failure: '500' } })
  });
  expect(queued.ok).toBe(true);

  await openUi(page, runtime);
  await page.getByRole('button', { name: 'Onboarding' }).click();
  await page.getByText('API-key and local providers').click();
  await page.getByLabel('CLIProxyAPI endpoint').fill(`${runtime.fakeBaseUrl}/cliproxyapi/v1`);
  await page.getByLabel('CLIProxyAPI credential').fill('webui-invalid-probe');

  const response = page.waitForResponse(value => value.url().endsWith('/admin/auth/cliproxyapi') && value.request().method() === 'POST');
  await page.getByRole('button', { name: 'Probe and connect CLIProxyAPI' }).click();
  expect((await response).status()).toBe(500);
  await expect(page.locator('#cliproxyapi-result')).toContainText('CLIProxyAPI connection failed');
  await expect(page.locator('#cliproxyapi-result')).toContainText('HTTP 500');
});
