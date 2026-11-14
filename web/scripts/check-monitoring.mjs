import assert from 'node:assert/strict';
import test from 'node:test';
import { normalizeIndexers, normalizeEvents, normalizeHealth, eventQuery, monitoringRequest } from '../src/lib/monitoring.ts';
test('normalizes wire variants without fabricating zero counters', () => {
  const [item] = normalizeIndexers({indexers: [{indexer_id:'a', name:'A', enabled:true, stats:{request_count:12, mean_response_ms:24}, circuit_breaker:{state:'open',failure_streak:3}}]});
  assert.equal(item.status,'circuit-open'); assert.equal(item.requests,12); assert.equal(item.failures,null); assert.equal(item.failureStreak,3);
  assert.equal(normalizeIndexers([{id:'a',enabled:false,status:'open'}])[0].status,'disabled');
  assert.throws(() => normalizeIndexers({error:'missing'}));
});
test('audit adapter drops raw queries and credentials', () => {
  const page = normalizeEvents({events:[{id:'1',indexer_id:'a',kind:'search',outcome:'success',created_at:'2026-01-01',query:'PRIVATE',url:'SECRET'}],total:1},25);
  assert.equal(page.items[0].timestamp,'2026-01-01'); assert.equal(page.total,1); assert.equal(page.hasMore,false);
  assert.ok(!JSON.stringify(page).includes('PRIVATE')); assert.ok(!JSON.stringify(page).includes('SECRET'));
});
test('all dependencies distinguish unconfigured from unknown', () => {
  const health = normalizeHealth({dependencies:{mail:{status:'unconfigured'},database:{status:'healthy'},connected_apps:{status:'degraded'}}});
  assert.equal(health.length,7); assert.equal(health.find(x=>x.id==='mail').status,'unconfigured'); assert.equal(health.find(x=>x.id==='systemTime').status,'unknown'); assert.equal(health.find(x=>x.id==='connectedApps').status,'degraded');
});
test('event cursors and time filters match the SQLite API contract', () => {
  const url = new URL(eventQuery({indexerId:'a&b',kind:'rss',outcome:'error',before:'2026-01-01T12:00',after:'2026-01-01T10:00'},25,25),'http://localhost');
  assert.equal(url.searchParams.get('indexerId'),'a&b');
  assert.equal(url.searchParams.get('before'),'25');
  assert.equal(url.searchParams.get('category'),'rss');
  assert.equal(url.searchParams.get('status'),'error');
  assert.ok(Number(url.searchParams.get('until')) > Number(url.searchParams.get('since')));
  assert.equal(url.searchParams.has('query'),false);
});
test('real backend response keeps counters and stable event cursors', () => {
  const stats = normalizeIndexers([{indexerId:'demo',displayName:'Demo',enabled:true,searches:8,failedQueries:2,authErrors:1,rss:3,downloads:4,averageResponseMs:125.5,consecutiveFailures:3,openUntil:Math.floor(Date.now()/1000)+90,lastActivity:1763140000}])[0];
  assert.equal(stats.name,'Demo');assert.equal(stats.requests,8); assert.equal(stats.failures,2);
  assert.equal(stats.authErrors,1); assert.equal(stats.rssOperations,3);assert.equal(stats.status,'circuit-open');
  const page = normalizeEvents({events:[{id:34,timestamp:1763140000,category:'search',status:'success',indexerId:'demo',query:'SECRET'}],nextBefore:34},25);
  assert.equal(page.items[0].id,'34'); assert.equal(page.items[0].kind,'search'); assert.equal(page.items[0].outcome,'success');
  assert.equal(page.nextBefore,34); assert.equal(page.hasMore,true);
  assert.ok(!JSON.stringify(page).includes('SECRET'));
});
test('health model accepts concrete component state, not just mock dependencies', () => {
  const values = normalizeHealth({database:'ok',indexers:[{enabled:true,status:'ok'}],components:{proxies:[],applications:[{enabled:true,status:'degraded'}],mail:{status:'unconfigured'},rssScheduler:[{enabled:true,status:'scheduled'}],clockSkew:{status:'unknown'}}});
  assert.equal(values.find(x=>x.id==='database').status,'healthy');
  assert.equal(values.find(x=>x.id==='connectedApps').status,'degraded');
  assert.equal(values.find(x=>x.id==='rssScheduler').status,'healthy');
  assert.equal(values.find(x=>x.id==='proxies').status,'unconfigured');
});
test('requests use bearer headers and cancellation; HTTP errors never echo bodies', async () => {
  const previous = globalThis.fetch; const controller = new AbortController();
  try {
    globalThis.fetch = async (url, options) => { assert.equal(url,'/api/v1/monitoring/health'); assert.equal(options.headers.Authorization,'Bearer private-token'); assert.ok(options.signal instanceof AbortSignal); assert.equal(options.signal.aborted,false); assert.equal(options.cache,'no-store'); return new Response('{"dependencies":{}}'); };
    await monitoringRequest('/api/v1/monitoring/health','private-token',controller.signal);
    globalThis.fetch = async () => new Response('SECRET',{status:404});
    await assert.rejects(monitoringRequest('/api/v1/monitoring/health','private-token',controller.signal), /HTTP 404/);
    globalThis.fetch = async () => new Response('<html>SPA fallback</html>');
    await assert.rejects(monitoringRequest('/api/v1/monitoring/health','private-token',controller.signal), /Unexpected monitoring response/);
  } finally { globalThis.fetch = previous; }
});
