import { test as base, expect, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import type { TestRuntime } from '../runtime';

export const test = base.extend<{ runtime: TestRuntime }>({
  runtime: async ({}, use) => {
    const runtime = JSON.parse(await readFile(path.resolve(__dirname, '..', '.runtime.json'), 'utf8')) as TestRuntime;
    await use(runtime);
  }
});

export { expect };

export async function openUi(page: Page, runtime: TestRuntime) {
  await page.goto(`${runtime.baseUrl}/ui`);
  await expect(page.locator('#daemon-status')).toContainText('Daemon ');
}
