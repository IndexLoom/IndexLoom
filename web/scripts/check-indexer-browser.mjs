import assert from 'node:assert/strict';

const { chromium } = await import(
  process.env.INDEXLOOM_PLAYWRIGHT_ENTRY ?? '/tmp/indexloom-playwright/node_modules/playwright/index.mjs'
);
const origin = process.env.INDEXLOOM_PREVIEW_URL ?? 'http://127.0.0.1:4173';
const fixture = {
  id: 'fixture-1337x', definitionId: '1337x', displayName: '1337x',
  enabled: true, catalogAvailable: true, privacy: 'public',
  userAgent: null, flaresolverrInstanceId: null, proxyId: null,
  groupIds: [], tags: ['anime'], lastTest: null,
  values: { sitelink: 'https://example.org/' }, configuredSecrets: [],
  capabilities: {
    description: 'Browser smoke fixture', language: 'en', source: 'jackett',
    searchModes: [{ name: 'search', parameters: [] }],
    categoryMappings: [{ id: '2000', category: 'Movies', description: 'Films', default: true }],
    categories: ['Movies', 'TV'], genreSearch: false, rawSearch: true,
    authentication: false, captcha: false, downloadHandler: true,
    searchPaths: 1, alternateUrls: 3,
  },
  configurationSchema: {
    fields: [{ name: 'sitelink', label: 'Site Link', kind: 'text', secret: false, options: [] }],
    flaresolverrRecommended: false, userAgentRecommended: false,
  },
};

const browser = await chromium.launch({ headless: true });
try {
  for (const width of [320, 390, 1440]) {
    const context = await browser.newContext({
      viewport: { width, height: 850 },
      isMobile: width < 641,
      hasTouch: width < 641,
    });
    try {
      const page = await context.newPage();
      const errors = [];
      let indexer = { ...fixture };
      let testStarted = 0;
      page.on('pageerror', error => errors.push(error.message));
      await page.route('**/api/v1/**', async route => {
        const request = route.request();
        const path = new URL(request.url()).pathname;
        const json = (body, status = 200) => route.fulfill({
          status, contentType: 'application/json', body: JSON.stringify(body),
        });
        if (path === '/api/v1/auth/config') return json({
          mode: 'disabled', localEnabled: false, oidcEnabled: false, mailEnabled: false,
          emailVerificationRequired: false, bootstrapAvailable: false, oidcIssuer: null,
        });
        if (path === '/api/v1/health') return json({ status: 'ok', version: 'test', database: 'ok' });
        if (path === '/api/v1/indexers' && request.method() === 'GET') return json([indexer]);
        if (['/api/v1/groups', '/api/v1/proxies', '/api/v1/flaresolverr'].includes(path)) return json([]);
        if (path === '/api/v1/routing') return json({ globalProxyId: null });
        if (path === '/api/v1/auth/me') return json({ method: 'disabled', user: null });
        if (path === '/api/v1/events/stream') return route.fulfill({ status: 204, body: '' });
        if (path === '/api/v1/search/presets') return json([]);
        if (path === '/api/v1/indexers/fixture-1337x/organization' && request.method() === 'PUT') {
          indexer = { ...indexer, ...request.postDataJSON() };
          return json(indexer);
        }
        if (path === '/api/v1/indexer-tests' && request.method() === 'POST') {
          assert.deepEqual(request.postDataJSON(), {
            all: false, indexerIds: ['fixture-1337x'], groupIds: [],
          });
          testStarted++;
          return json({
            id: 'fixture-test', status: 'completed', total: 1, completed: 1,
            results: [{
              indexerId: 'fixture-1337x', indexerName: '1337x',
              success: true, responseTimeMs: 242, message: 'Fixture passed',
            }],
            createdAt: '2026-11-14T20:00:00Z', finishedAt: '2026-11-14T20:00:01Z',
          });
        }
        return json({ message: 'Unexpected mock API call: ' + request.method() + ' ' + path }, 404);
      });

      await page.goto(origin + '/?view=indexers', { waitUntil: 'domcontentloaded' });
      const row = page.locator('.il-indexer-row').first();
      await row.getByRole('link', { name: 'Search' }).waitFor();
      const dimensions = await page.evaluate(() => ({
        documentWidth: document.documentElement.scrollWidth,
        viewportWidth: document.documentElement.clientWidth,
      }));
      assert.ok(dimensions.documentWidth <= dimensions.viewportWidth + 5,
        'Overflow at ' + width + 'px: ' + JSON.stringify(dimensions));

      await row.getByRole('button', { name: 'Configure' }).click();
      const modal = page.locator('dialog.il-config-dialog');
      await modal.waitFor({ state: 'visible' });
      await modal.getByText('Site settings', { exact: true }).first().waitFor();
      assert.equal(await modal.locator('input[type="checkbox"]').first().evaluate(
        input => getComputedStyle(input).appearance
      ), 'none', 'Browser default checkbox leaked');
      await page.keyboard.press('Escape');
      await modal.waitFor({ state: 'hidden' });

      await row.getByRole('button', { name: 'Test', exact: true }).click();
      await row.getByText('Fixture passed', { exact: false }).waitFor();
      assert.equal(testStarted, 1, 'Single indexer test did not start exactly once');

      await row.getByRole('button', { name: 'Disable' }).click();
      await row.getByRole('button', { name: 'Enable' }).waitFor();
      assert.equal(indexer.enabled, false);
      assert.equal(await row.getByRole('link', { name: 'Search' }).count(), 0);
      await row.getByRole('button', { name: 'Enable' }).click();
      await row.getByRole('link', { name: 'Search' }).waitFor();
      assert.equal(indexer.enabled, true);

      await row.getByRole('link', { name: 'Search' }).click();
      await page.getByRole('heading', { name: 'Search', exact: true }).waitFor();
      const url = new URL(page.url());
      assert.equal(url.searchParams.get('view'), 'search');
      assert.equal(url.searchParams.get('indexer'), 'fixture-1337x');
      assert.deepEqual(errors, [], 'Uncaught browser errors at ' + width + 'px');
      console.log('Indexer UI browser smoke passed at ' + width + 'px');
    } finally {
      await context.close();
    }
  }
} finally {
  await browser.close();
}
