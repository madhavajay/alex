import { test, expect, openUi } from './fixtures';

test('providers show an upstream-backed CLIProxyAPI error instead of a blank state', async ({ page, runtime }) => {
  const queued = await fetch(`${runtime.fakeBaseUrl}/_control/queue`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-control-key': runtime.fakeControlKey },
    body: JSON.stringify({ endpoint: 'GET /cliproxyapi/v1/models', response: { failure: '500' } })
  });
  expect(queued.ok).toBe(true);

  await openUi(page, runtime);
  await page.locator('nav [data-view="providers"]').click();
  await page.locator('#provider-tabs [data-section-tab="cliproxyapi"]').click();
  const setup = page.locator('[data-provider-setup="cliproxyapi"]');
  await setup.locator('summary').click();
  await setup.getByLabel('Endpoint URL').fill(`${runtime.fakeBaseUrl}/cliproxyapi/v1`);
  await setup.getByLabel('Credential').fill('webui-invalid-probe');

  const response = page.waitForResponse(value => value.url().endsWith('/admin/auth/cliproxyapi') && value.request().method() === 'POST');
  await setup.getByRole('button', { name: 'Probe and connect' }).click();
  expect((await response).status()).toBe(500);
  await expect(page.locator('#cliproxyapi-result')).toContainText('CLIProxyAPI connection failed');
  await expect(page.locator('#cliproxyapi-result')).toContainText('HTTP 500');
});
