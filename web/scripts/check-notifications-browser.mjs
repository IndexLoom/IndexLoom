// Optional browser tooling: INDEXLOOM_PLAYWRIGHT_ENTRY may point at an existing Playwright install.
import assert from 'node:assert/strict';
const { chromium } = await import(process.env.INDEXLOOM_PLAYWRIGHT_ENTRY ?? '../node_modules/playwright/index.mjs');
const origin = process.env.INDEXLOOM_PREVIEW_URL ?? 'http://127.0.0.1:4175';
const eventIds = ['system.startup', 'auth.login', 'automation.failed', 'automation.completed', 'indexer.test.failed', 'indexer.test.passed'];
const browser = await chromium.launch({ headless: true, ...(process.env.INDEXLOOM_CHROMIUM_PATH ? { executablePath: process.env.INDEXLOOM_CHROMIUM_PATH } : {}) });
try {
  for (const width of [320, 390, 1440]) {
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    let channels = [], templates = [], deliveries = [], lastWrite, testFails = false;
    await page.route('**/api/v1/**', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method();
      const json = (body, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
      if (path === '/api/v1/auth/config') return json({ mode: 'disabled', localEnabled: false, oidcEnabled: false, mailEnabled: false, emailVerificationRequired: false, bootstrapAvailable: false, oidcIssuer: null });
      if (path === '/api/v1/auth/me') return json({ method: 'disabled', user: null });
      if (path === '/api/v1/health') return json({ status: 'ok', database: 'ok', version: 'test' });
      if (path === '/api/v1/events/stream') return route.fulfill({ status: 204, body: '' });
      if (path === '/api/v1/notifications/events') return json(eventIds.map(id => ({ id, title: id, message: 'Event message' })));
      if (path === '/api/v1/notifications/templates' && method === 'GET') return json(templates);
      if (path === '/api/v1/notifications/deliveries') return json(deliveries);
      if (path === '/api/v1/notifications/channels' && method === 'GET') return json(channels);
      if ((path === '/api/v1/notifications/channels' && method === 'POST') || (path === '/api/v1/notifications/channels/channel-1' && method === 'PUT')) {
        lastWrite = request.postDataJSON();
        const { secrets, ...publicFields } = lastWrite;
        const c = { ...publicFields, id: 'channel-1', revision: (channels[0]?.revision ?? 0) + 1, storedSecrets: ['token'] };
        channels = [c]; return json(c);
      }
      if (path === '/api/v1/notifications/channels/channel-1/test') {
        deliveries = [{ id: 'delivery-1', channelId: 'channel-1', event: 'test', status: testFails ? 'failed' : 'sent', attempts: 1, summary: testFails ? 'Provider rejected request' : 'Provider accepted notification', durationMs: 12, isTest: true, createdAt: 1794700800, nextAttempt: 1794700800 }];
        return json({ success: !testFails, status: testFails ? 'failed' : 'sent', summary: testFails ? 'Provider rejected request' : 'Provider accepted notification', durationMs: 12 });
      }
      if (path.startsWith('/api/v1/notifications/templates/') && method === 'PUT') {
        const body = request.postDataJSON(); templates = [{ ...body, scope: url.searchParams.get('channelId') ?? '', event: path.split('/').at(-1) }]; return json(templates[0]);
      }
      if (path.startsWith('/api/v1/notifications/templates/') && method === 'DELETE') { templates = []; return json({ reset: true }); }
      if (path === '/api/v1/notifications/preview') { const body = request.postDataJSON(); return json({ title: body.title.replace('{{app}}', 'IndexLoom'), body: body.body.replace('{{event}}', body.event) }); }
      if (path === '/api/v1/notifications/channels/channel-1' && method === 'DELETE') { channels = []; deliveries = []; return json({ deleted: true }); }
      return json([], 200);
    });
    await page.goto(origin + '/?view=settings', { waitUntil: 'domcontentloaded' });
    assert.match(await page.title(), /IndexLoom/);
    await page.getByRole('tab', { name: 'Notifications', exact: true }).click();
    const panel = page.locator('.notifications-settings');
    await panel.getByText('No channels. External notifications are off until you add one.').waitFor();
    await panel.getByRole('button', { name: 'Add channel', exact: true }).click();
    await panel.getByLabel('Channel name', { exact: true }).fill('Operations');
    for (const provider of ['discord', 'slack', 'webhook', 'ntfy', 'gotify', 'apprise', 'pushover', 'telegram']) {
      await panel.getByRole('combobox', { name: 'Provider', exact: true }).selectOption(provider);
      if (['discord', 'slack', 'webhook', 'apprise'].includes(provider)) await panel.getByLabel('Secret endpoint URL', { exact: true }).waitFor();
      if (provider === 'ntfy') await panel.getByLabel('Topic', { exact: true }).waitFor();
      if (provider === 'gotify') await panel.getByLabel('Public API base URL', { exact: true }).waitFor();
      if (provider === 'pushover') await panel.getByLabel('Pushover user key', { exact: true }).waitFor();
    }
    await panel.getByRole('combobox', { name: 'Telegram Bot API', exact: true }).selectOption('selfhosted');
    assert.equal(await panel.getByLabel('Public API base URL', { exact: true }).inputValue(), 'http://127.0.0.1:8081');
    assert.equal(await panel.getByLabel('Allow HTTP for localhost or private IPs', { exact: true }).isChecked(), true);
    await panel.getByLabel('Chat ID', { exact: true }).fill('-100123');
    await panel.getByLabel('Thread ID (optional)', { exact: true }).fill('42');
    await panel.getByLabel('API token', { exact: true }).fill('123:dummy-secret');
    await panel.getByLabel('Silent Telegram message', { exact: true }).check();
    const checkbox = panel.getByLabel('Enable channel', { exact: true });
    assert.equal(await checkbox.evaluate(e => getComputedStyle(e).appearance), 'none');
    await panel.getByRole('button', { name: 'Save channel', exact: true }).click();
    await panel.getByText('Notification channel saved', { exact: true }).waitFor();
    assert.equal(lastWrite.config.threadId, 42); assert.equal(lastWrite.config.disableNotification, true);
    await panel.getByRole('button', { name: 'Edit Operations', exact: true }).click();
    const secret = panel.getByLabel('API token', { exact: true });
    assert.equal(await secret.inputValue(), ''); assert.equal(await secret.getAttribute('placeholder'), 'Stored · leave blank to keep');
    await panel.getByRole('button', { name: 'Save channel', exact: true }).click();
    await panel.getByText('Notification channel saved', { exact: true }).waitFor(); assert.deepEqual(lastWrite.secrets, {});
    await panel.getByRole('button', { name: 'Send test Operations', exact: true }).click();
    await panel.getByText('Test accepted by provider', { exact: true }).waitFor(); await panel.getByText('Sent', { exact: true }).waitFor();
    testFails = true; await panel.getByRole('button', { name: 'Send test Operations', exact: true }).click();
    await panel.getByRole('alert').filter({ hasText: 'Test failed: Provider rejected request' }).waitFor();
    await panel.locator('summary').filter({ hasText: 'Event templates' }).click();
    await panel.getByRole('combobox', { name: 'Template scope', exact: true }).selectOption('channel-1');
    await panel.getByRole('combobox', { name: 'Event', exact: true }).selectOption('auth.login');
    await panel.getByLabel('Notification title', { exact: true }).fill('<img src=x> {{app}}');
    await panel.getByRole('textbox', { name: 'Notification body', exact: true }).fill('{{event}}');
    await panel.getByRole('button', { name: 'Preview', exact: true }).click();
    await panel.locator('output').getByText('<img src=x> IndexLoom', { exact: true }).waitFor(); assert.equal(await panel.locator('output img').count(), 0);
    await panel.getByRole('button', { name: 'Save template', exact: true }).click();
    await panel.getByText('Notification template saved', { exact: true }).waitFor(); assert.equal(templates[0].scope, 'channel-1');
    const dimensions = await page.evaluate(() => ({ width: document.documentElement.scrollWidth, viewport: document.documentElement.clientWidth }));
    assert.ok(dimensions.width <= dimensions.viewport + 1, 'Overflow at ' + width + ': ' + JSON.stringify(dimensions));
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: '/tmp/indexloom-notifications-' + width + '.png', fullPage: false });
    await panel.getByRole('button', { name: 'Delete Operations', exact: true }).click();
    await panel.getByRole('button', { name: 'Confirm delete', exact: true }).click();
    await panel.getByText('No channels. External notifications are off until you add one.').waitFor();
    assert.deepEqual(errors, []); assert.equal(await page.locator('vite-error-overlay').count(), 0);
    console.log('Notifications browser flow passed at ' + width + 'px');
    await context.close();
  }
  // Role visibility is checked independently of the server ACL tests.
  const context = await browser.newContext(); const page = await context.newPage();
  await page.addInitScript(() => sessionStorage.setItem('indexloom.admin-token.v1', 'viewer-fixture-token'));
  await page.route('**/api/v1/**', route => {
    const path = new URL(route.request().url()).pathname;
    const body = path === '/api/v1/auth/config' ? { mode: 'local', localEnabled: true, oidcEnabled: false, mailEnabled: false, emailVerificationRequired: false, bootstrapAvailable: false }
      : path === '/api/v1/auth/me' ? { method: 'session', user: { role: 'viewer', id: 'viewer' } } : [];
    return route.fulfill({ contentType: 'application/json', body: JSON.stringify(body) });
  });
  await page.goto(origin + '/?view=settings'); await page.getByRole('tab', { name: 'Appearance', exact: true }).waitFor();
  assert.equal(await page.getByRole('tab', { name: 'Notifications', exact: true }).count(), 0);
  await context.close(); console.log('Owner-only settings visibility passed');
} finally { await browser.close(); }
