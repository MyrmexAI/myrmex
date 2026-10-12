// Чтение файлов агентом (toolloop.ts): картинка уходит модели изображением, а не текстом;
// прочие бинарные файлы — только размер и хеш (раньше PNG «текстом» сжигал сотни тысяч токенов).

import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import type { AssignmentPayload } from '@swarm/protocol';
import { ToolLoopRunner } from '../src/toolloop.ts';

test('read_file: PNG — изображением в следующем сообщении, бинарный файл — описанием, текст — текстом', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'loop-'));
  mkdirSync(join(dir, 'artifacts'));
  const png = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13]), Buffer.alloc(200, 7)]);
  writeFileSync(join(dir, 'artifacts/image.png'), png);
  writeFileSync(join(dir, 'artifacts/sound.mp3'), Buffer.from([0xff, 0xfb, 0, 0, 1, 2, 3]));
  writeFileSync(join(dir, 'notes.txt'), 'hello');

  const calls: any[] = [];
  const replies = [
    { tool_calls: ['artifacts/image.png', 'artifacts/sound.mp3', 'notes.txt'].map((path, i) => ({ id: `c${i}`, type: 'function', function: { name: 'read_file', arguments: JSON.stringify({ path }) } })) },
    { tool_calls: [{ id: 'f', type: 'function', function: { name: 'finish', arguments: JSON.stringify({ summary: 'ok', verdict: 'accept', findings: [] }) } }] },
  ];
  const fetchImpl = (async (_url: string, init: any) => {
    calls.push(JSON.parse(init.body));
    return new Response(JSON.stringify({ choices: [{ message: { role: 'assistant', content: null, ...replies.shift() }, finish_reason: 'tool_calls' }], usage: { prompt_tokens: 10, completion_tokens: 5 } }));
  }) as typeof fetch;

  const a = { v: 1, leaseId: 'l', jobId: 'j', nodeId: 'n', skill: 'adversarial-review', skillHash: 'h', role: 'review', objective: 'check', acceptanceCriteria: [], references: [], inputs: [], source: null, reads: [], paths: [], outputs: [], deadline: '', limits: { modelTokens: 1, wallClockSeconds: 1, diskMb: 1 }, model: 'm' } as unknown as AssignmentPayload;
  await new ToolLoopRunner({ apiKey: 'k', model: 'm', fetchImpl }).run(dir, a, new AbortController().signal);

  const msgs = calls[1].messages as any[];
  const tools = msgs.filter((m) => m.role === 'tool');
  assert.match(tools[0].content, /^image artifacts\/image\.png: 212 bytes \(image\/png\)/);
  assert.match(tools[1].content, /^binary file artifacts\/sound\.mp3: 7 bytes, sha256 [0-9a-f]{64}/);
  assert.equal(tools[2].content, 'hello');
  const img = msgs.at(-1);
  assert.equal(img.role, 'user');
  assert.equal(img.content[1].type, 'image_url');
  assert.equal(img.content[1].image_url.url, `data:image/png;base64,${png.toString('base64')}`);
  assert.ok(JSON.stringify(msgs).length < 5_000, 'никаких сотен килобайт текста');
});

test('ответ Claude Code без result.json: вердикт и находки — из последнего JSON в тексте', async () => {
  const { lastResultJson } = await import('../src/runner.ts');
  const text = 'I looked at the image {not json}.\n```json\n{"summary": "Pink ant logo, matches", "verdict": "accept", "findings": [{"severity": "info", "title": "ok"}]}\n```';
  assert.deepEqual(lastResultJson(text), { summary: 'Pink ant logo, matches', verdict: 'accept', findings: [{ severity: 'info', title: 'ok' }] });
  assert.equal(lastResultJson('no json here'), null);
});
