const assert = require('node:assert/strict');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

async function openDashboard(browser, width) {
  const page = await browser.newPage({ viewport: { width, height: 900 } });
  const clients = [
    ['10.0.0.4', 'Device4'],
    ['10.0.0.42', 'Device42'],
    ['2001:db8::1', 'IPv6Prefix'],
    ['2001:db8::12', 'IPv6Device'],
    ['10.0.0.7', 'Shared'],
    ['10.0.0.8', 'Shared'],
  ].map(([client, name]) => ({ client, name, queries: 1, last_seen_ms: Date.now() }));
  const queryRequests = [];
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/api/clients?**', route => route.fulfill({ json: clients }));
  await page.route('**/api/queries?**', route => {
    const url = new URL(route.request().url());
    queryRequests.push(url.searchParams);
    const until = Number(url.searchParams.get('until_ms')) || Date.now();
    return route.fulfill({ json: {
      range: { from_ms: until - 3600000, until_ms: until },
      total: 0,
      entries: [],
    } });
  });
  await page.goto(process.env.DNS_DASHBOARD_URL || 'http://127.0.0.1:18153');
  await page.getByLabel('Master password').fill(process.env.DNS_PASSWORD || 'dev');
  await page.getByRole('button', { name: 'Log in', exact: true }).click();
  await page.locator('#client-filter-options option').first().waitFor({ state: 'attached' });
  return { page, queryRequests, errors };
}

async function sequentialAddresses({ page }) {
  const client = page.getByLabel('Client name or IP', { exact: true });
  await client.pressSequentially('10.0.0.42', { delay: 40 });
  assert.equal(await client.inputValue(), '10.0.0.42', 'Typing must not replace a valid address prefix with its device label');
  assert.equal(new URL(page.url()).searchParams.get('client'), null, 'An address must not be selected before Enter or blur');
  await client.press('Enter');
  await page.waitForFunction(() => new URL(location.href).searchParams.get('client') === '10.0.0.42');
  assert.equal(await client.inputValue(), 'Device42 (10.0.0.42)');
  await client.fill('');
  await client.pressSequentially('2001:db8::12', { delay: 25 });
  assert.equal(await client.inputValue(), '2001:db8::12');
  assert.equal(new URL(page.url()).searchParams.get('client'), null);
  await page.getByLabel('Domain', { exact: true }).focus();
  await page.waitForFunction(() => new URL(location.href).searchParams.get('client') === '2001:db8::12');
  assert.equal(await client.inputValue(), 'IPv6Device (2001:db8::12)');
}

async function combinedFilters({ page, queryRequests }) {
  const domain = page.getByLabel('Domain', { exact: true });
  const client = page.getByLabel('Client name or IP', { exact: true });
  const combinedRequest = page.waitForRequest(request => {
    const query = new URL(request.url());
    return query.pathname === '/api/queries' && query.searchParams.get('domain') === 'example.test' && query.searchParams.get('client') === '10.0.0.42';
  });
  combinedRequest.catch(() => {});
  await domain.fill('example.test');
  await client.fill('Device42 (10.0.0.42)');
  await page.waitForTimeout(400);
  assert.equal(await domain.inputValue(), 'example.test', 'Client selection must not discard a pending domain');
  const selection = new URL(page.url()).searchParams;
  assert.equal(selection.get('domain'), 'example.test');
  assert.equal(selection.get('client'), '10.0.0.42');
  await combinedRequest;
  assert(queryRequests.some(query => query.get('domain') === 'example.test' && query.get('client') === '10.0.0.42'));
  await domain.fill('replacement.test');
  await client.fill('Shared');
  await client.press('Enter');
  await page.waitForTimeout(400);
  assert.equal(new URL(page.url()).searchParams.get('client'), '10.0.0.42', 'Ambiguous names must not change the applied client');
  assert.equal(new URL(page.url()).searchParams.get('domain'), 'replacement.test');
  await domain.fill('discard.test');
  await page.getByRole('button', { name: 'Reset filters', exact: true }).click();
  await page.waitForTimeout(400);
  assert.equal(await domain.inputValue(), '');
  assert.equal(await client.inputValue(), '');
  assert.equal(new URL(page.url()).searchParams.get('domain'), null);
  assert.equal(new URL(page.url()).searchParams.get('client'), null);
}

async function emptyExactHistory({ page, queryRequests }) {
  const mode = page.getByLabel('Domain match', { exact: true });
  await mode.selectOption('exact');
  await page.waitForTimeout(100);
  const previousRequests = queryRequests.length;
  await page.reload();
  await mode.waitFor();
  assert.equal(await mode.inputValue(), 'exact', 'Reload must preserve Exact even without a domain');
  await page.locator('#client-filter-options option').first().waitFor({ state: 'attached' });
  assert.equal(queryRequests.length, previousRequests + 1);
  const beforeHistory = queryRequests.length;
  await mode.selectOption('contains');
  await page.waitForTimeout(100);
  await page.goBack();
  await page.waitForFunction(() => document.querySelector('[aria-label="Domain match"]').value === 'exact');
  await page.goForward();
  await page.waitForFunction(() => document.querySelector('[aria-label="Domain match"]').value === 'contains');
  await page.goBack();
  await page.waitForFunction(() => document.querySelector('[aria-label="Domain match"]').value === 'exact');
  assert.equal(queryRequests.length, beforeHistory, 'Inactive mode history changes must not reload queries');
  const applied = page.waitForResponse(response => response.url().includes('/api/queries?') && response.url().includes('domain_exact=true') && response.url().includes('domain=example.test'));
  await page.getByLabel('Domain', { exact: true }).fill('example.test');
  await applied;
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const failures = [];
  try {
    for (const width of [1440, 390]) {
      for (const test of [sequentialAddresses, combinedFilters, emptyExactHistory]) {
        const context = await openDashboard(browser, width);
        try {
          await test(context);
          assert.deepEqual(context.errors, []);
          console.log(`PASS: ${test.name} (${width}px)`);
        } catch (error) {
          failures.push(`${test.name} (${width}px): ${error.message}`);
        } finally {
          await context.page.close();
        }
      }
    }
    assert.deepEqual(failures, []);
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
