import test from 'node:test';
import assert from 'node:assert/strict';
import { SseParser } from '../src/lib/events-parser.ts';
test('SSE handles fragmented CRLF, comments, multiline data and IDs', () => {
  const events = [];
  const parser = new SseParser((...event) => events.push(event));
  for (const chunk of [': keepalive\r\n\r\n', 'id: epoch:2\r\nevent: cha', 'nge\r\ndata: {"kind":\r\n', 'data: "indexers"}\r\n\r', '\n']) parser.push(chunk);
  assert.deepEqual(events, [['change', 'epoch:2', '{"kind":\n"indexers"}']]);
});
test('SSE rejects oversized unfinished frames', () => {
  assert.throws(() => new SseParser(() => {}).push('x'.repeat(65537)), /limit/);
});
