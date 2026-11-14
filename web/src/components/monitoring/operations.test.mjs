import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { createServer } from 'vite';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

// Load the real TSX and localization API through the same compiler as the app.
const server = await createServer({ server: { middlewareMode: true, hmr: false, ws: false }, configFile: false, esbuild: { jsx: 'automatic' }, appType: 'custom' });
after(() => server.close());
globalThis.window = { localStorage: { getItem: key => key === 'indexloom.locale.v1' ? 'ru' : null } };
after(() => { delete globalThis.window; });
const { translate } = await server.ssrLoadModule('/src/lib/i18n/index.tsx');
const { DiagnosticsTable, backupSize, queueBackupRestore } = await server.ssrLoadModule('/src/components/ManagementSettings.tsx');
const { AuditTable } = await server.ssrLoadModule('/src/components/monitoring/AuditTable.tsx');

test('diagnostics count substitutes the real variable for empty and populated results', () => {
  for (const count of [0, 1, 100]) {
    const text = translate('{{count}} events displayed', { count }, 'settings');
    assert.ok(text.includes(String(count)));
    assert.ok(!text.includes('{'));
  }
});

test('diagnostics renders semantic columns, localized severity and safe runtime messages', () => {
  const html = renderToStaticMarkup(createElement(DiagnosticsTable, { logs: [{ id: 1, timestamp: '2026-11-15T12:00:00Z', severity: 'warn', component: 'scheduler', message: '<script>untrusted</script>\nsecond line' }] }));
  assert.equal((html.match(/scope="col"/g) ?? []).length, 4);
  assert.match(html, /<time dateTime="2026-11-15T12:00:00Z"/);
  assert.match(html, /Предупреждение/);
  assert.match(html, /Планировщик/);
  assert.match(html, /&lt;script&gt;untrusted&lt;\/script&gt;/);
  assert.ok(!html.includes('<script>'));
});

test('backup sizes preserve small values and show MiB rather than raw bytes', () => {
  assert.match(backupSize(1048576), /^1 /);
  assert.match(backupSize(262144), /0[,.]25 /);
  assert.match(backupSize(268435456), /^256 /);
});

test('audit invalid date range announces error and suppresses misleading rows', () => {
  const html = renderToStaticMarkup(createElement(AuditTable, {
    filters: { indexerId: '', kind: '', outcome: '', before: '2026-01-01T10:00', after: '2026-01-02T10:00' },
    onFilter() {}, indexers: [], page: { items: [{ id: '1', indexerName: 'Hidden row', timestamp: '', kind: '', outcome: '' }], hasMore: false },
    loading: false, invalidDates: true, pageIndex: 0, onPrevious() {}, onNext() {},
  }));
  assert.match(html, /role="alert"/);
  assert.match(html, /aria-invalid="true"/);
  assert.ok(!html.includes('Hidden row'));
});

test('restore requires the exact confirmation and queues only the named backup', async () => {
  const original = globalThis.fetch;
  const requests = [];
  globalThis.fetch = async (path, init) => { requests.push({ path, init }); return new Response(null, { status: 202 }); };
  try {
    for (const answer of [null, '', 'RESTORE', 'restore on restart']) {
      assert.equal(await queueBackupRestore('test-token', 'snapshot.db', () => answer), false);
    }
    assert.equal(requests.length, 0);
    assert.equal(await queueBackupRestore('test-token', 'snapshot.db', message => {
      assert.ok(message.includes('snapshot.db'));
      assert.ok(!message.includes('{{id}}'));
      return 'RESTORE ON RESTART';
    }), true);
    assert.equal(requests.length, 1);
    assert.equal(requests[0].path, '/api/v1/backups/snapshot.db/restore');
    assert.equal(requests[0].init.method, 'POST');
    assert.deepEqual(JSON.parse(requests[0].init.body), { confirmation: 'RESTORE ON RESTART' });
  } finally { globalThis.fetch = original; }
});
