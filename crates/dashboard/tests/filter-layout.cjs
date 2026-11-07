const assert = require('node:assert/strict');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

async function checkLayout(browser, width) {
  const page = await browser.newPage({ viewport: { width, height: 900 } });
  const errors = [];
  const queryModes = [];
  let statsRequests = 0;
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    const url = new URL(request.url());
    if (url.pathname === '/api/queries') queryModes.push(url.searchParams.get('domain_exact'));
    if (url.pathname === '/api/stats') statsRequests += 1;
  });
  const entries = Array.from({ length: 25 }, (_, index) => ({
    timestamp_ms: Date.now() - index * 1000,
    client: '127.0.0.1',
    client_name: 'Laptop',
    protocol: 'udp',
    domain: 'example.test.',
    record_type: 'A',
    response_code: 'NOERROR',
    latency_ms: 1,
    outcome: { status: 'answered', source: { kind: 'cache' } },
  }));
  await page.route('**/api/clients?**', route => route.fulfill({ json: [] }));
  await page.route('**/api/queries?**', async route => {
    await new Promise(resolve => setTimeout(resolve, 250));
    const url = new URL(route.request().url());
    const until = Number(url.searchParams.get('until_ms')) || Date.now();
    await route.fulfill({ json: {
      range: { from_ms: until - 3600000, until_ms: until },
      total: url.searchParams.has('domain') ? entries.length : 75,
      entries,
    } });
  });
  await page.goto(`${process.env.DNS_DASHBOARD_URL || 'http://127.0.0.1:18153'}/?domain=example.test`);
  await page.getByLabel('Master password').fill(process.env.DNS_PASSWORD || 'dev');
  await page.getByRole('button', { name: 'Log in', exact: true }).click();
  await page.waitForFunction(() => document.querySelectorAll('.query-table tbody tr').length === 25);
  await page.evaluate(() => {
    const filter = document.querySelector('.domain-filter');
    scrollTo(0, filter.getBoundingClientRect().top + scrollY - 150);
  });
  await page.waitForTimeout(100);

  for (const mode of ['exact', 'contains']) {
    await page.evaluate(() => {
      window.layoutSamples = [];
      const sample = () => {
        const panel = document.querySelector('#queries');
        window.layoutSamples.push({
          height: panel.getBoundingClientRect().height,
          scroll: scrollY,
          rows: panel.querySelectorAll('tbody tr').length,
        });
        window.layoutFrame = requestAnimationFrame(sample);
      };
      sample();
    });
    queryModes.length = 0;
    const exact = mode === 'exact' ? 'true' : 'false';
    const refreshed = page.waitForResponse(response => response.url().includes('/api/queries?') && response.url().includes(`domain_exact=${exact}`));
    await page.getByLabel('Domain match', { exact: true }).selectOption(mode);
    await page.getByText('Updating queries…', { exact: true }).waitFor();
    assert(await page.locator('.query-results').evaluate(element => element.inert));
    assert(await page.getByRole('button', { name: 'Pause live log', exact: true }).isDisabled());
    await refreshed;
    await page.waitForFunction(() => document.querySelector('.query-results')?.getAttribute('aria-busy') === 'false');
    await page.waitForTimeout(100);
    const samples = await page.evaluate(() => {
      cancelAnimationFrame(window.layoutFrame);
      return window.layoutSamples;
    });
    const baseline = samples[0];
    const measured = {
      width,
      mode,
      initialHeight: baseline.height,
      minimumHeight: Math.min(...samples.map(sample => sample.height)),
      minimumRows: Math.min(...samples.map(sample => sample.rows)),
      maximumScrollDelta: Math.max(...samples.map(sample => Math.abs(sample.scroll - baseline.scroll))),
    };
    console.log(JSON.stringify(measured));
    assert.equal(measured.minimumRows, entries.length, 'Changing the domain match mode must not remove the current rows while loading');
    assert(measured.minimumHeight >= baseline.height - 1, 'The query panel must not collapse while loading');
    assert(measured.maximumScrollDelta <= 1, 'Changing the domain match mode must not move the viewport');
    assert.deepEqual(queryModes, [exact], 'A mode change must issue only one query request');
    assert.equal(await page.locator('.query-results').evaluate(element => element.inert), false);
  }

  queryModes.length = 0;
  await page.getByRole('button', { name: 'Pause live log', exact: true }).click();
  await page.waitForFunction(() => new URL(location.href).searchParams.has('until_ms') && document.querySelector('.query-results')?.getAttribute('aria-busy') === 'false');
  assert(await page.getByRole('button', { name: 'Resume live log', exact: true }).isEnabled());
  await page.waitForTimeout(300);
  assert.deepEqual(queryModes, [], 'Freezing the displayed snapshot must not reload its rows');
  const resumed = page.waitForResponse(response => response.url().includes('/api/queries?'));
  await page.getByRole('button', { name: 'Resume live log', exact: true }).click();
  await resumed;
  await page.waitForFunction(() => document.querySelector('.query-results')?.getAttribute('aria-busy') === 'false');
  assert(await page.getByRole('button', { name: 'Pause live log', exact: true }).isEnabled());
  const cutoff = Date.now() - 1000;
  await page.goto(`${process.env.DNS_DASHBOARD_URL || 'http://127.0.0.1:18153'}/?offset=50&until_ms=${cutoff}`);
  await page.locator('.query-results[aria-busy="false"]').waitFor();
  await page.waitForTimeout(100);
  queryModes.length = 0;
  statsRequests = 0;
  for (const mode of ['exact', 'contains', 'exact']) {
    await page.getByLabel('Domain match', { exact: true }).selectOption(mode);
    await page.waitForTimeout(100);
    assert.equal(await page.getByLabel('Domain match', { exact: true }).inputValue(), mode);
    assert.equal(await page.locator('.query-results').getAttribute('aria-busy'), 'false');
    const selection = new URL(page.url()).searchParams;
    assert.equal(selection.get('offset'), '50');
    assert.equal(selection.get('until_ms'), String(cutoff));
  }
  assert.deepEqual(queryModes, [], 'Empty-domain mode changes must not reload queries or reset pagination');
  assert.equal(statsRequests, 0, 'Empty-domain mode changes must not restart stats polling');
  const filtered = page.waitForResponse(response => response.url().includes('/api/queries?') && response.url().includes('domain_exact=true') && response.url().includes('domain=example.test'));
  await page.getByLabel('Domain', { exact: true }).fill('example.test');
  await filtered;
  await page.locator('.query-results[aria-busy="false"]').waitFor();
  assert.equal(new URL(page.url()).searchParams.get('offset'), '0');
  assert.deepEqual(queryModes, ['true'], 'Entering a domain must apply the previously selected exact mode');
  assert.deepEqual(errors, []);
  await page.close();
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const width of [1440, 390]) await checkLayout(browser, width);
    console.log('PASS: stable filter layout and pause/resume; empty-domain mode changes avoid requests and preserve pagination; typing applies the selected mode.');
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
