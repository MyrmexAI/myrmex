// Сбор результата: изменения, которые агент закоммитил сам, тоже попадают в bundle.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { collect, git, prepare } from '../src/workspace.ts';

const ID = ['-c', 'user.name=t', '-c', 'user.email=t@t', '-c', 'commit.gpgsign=false'];
const assignment = (paths: string[]) =>
  ({ v: 1, leaseId: 'l1', jobId: 'j', nodeId: 'n1234567', skill: 'write-program-tests', role: 'implement', objective: 'x', acceptanceCriteria: [], references: [], inputs: [], source: null, reads: [], outputs: [], paths }) as any;

test('collect: agent-committed changes produce a bundle', async () => {
  const a = assignment([]);
  const p = await prepare(mkdtempSync(join(tmpdir(), 'ws-')), a, async () => Buffer.alloc(0));
  writeFileSync(join(p.dir, 'lib.rs'), 'fn main() {}\n');
  await git(p.dir, ...ID, 'add', 'lib.rs');
  await git(p.dir, ...ID, 'commit', '--quiet', '-m', 'agent commit');
  const c = await collect(p, a);
  assert.ok(c.bundle && c.bundle.length > 0, 'bundle expected');
  assert.equal(c.headCommit, await git(p.dir, 'rev-parse', 'HEAD'));
});

test('collect: committed files outside the allowed paths are refused', async () => {
  const a = assignment(['programs/']);
  const p = await prepare(mkdtempSync(join(tmpdir(), 'ws-')), a, async () => Buffer.alloc(0));
  writeFileSync(join(p.dir, 'evil.txt'), 'x');
  await git(p.dir, ...ID, 'add', 'evil.txt');
  await git(p.dir, ...ID, 'commit', '--quiet', '-m', 'agent commit');
  await assert.rejects(collect(p, a), /outside the allowed/);
});

test('collect: tracked dist/ of an imported site is committed although build dirs are excluded', async () => {
  const a = { ...assignment([]), skill: 'build-website' };
  const p = await prepare(mkdtempSync(join(tmpdir(), 'ws-')), a, async () => Buffer.alloc(0));
  const { mkdirSync } = await import('node:fs');
  mkdirSync(join(p.dir, 'dist'), { recursive: true });
  writeFileSync(join(p.dir, 'dist', 'index.html'), '<p>v1</p>');
  await git(p.dir, ...ID, 'add', '-f', 'dist/index.html');
  await git(p.dir, ...ID, 'commit', '--quiet', '-m', 'site v1');
  const base = await git(p.dir, 'rev-parse', 'HEAD');
  writeFileSync(join(p.dir, 'dist', 'index.html'), '<p>v2</p>');
  const c = await collect({ ...p, baseCommit: base }, a);
  assert.ok(c.bundle, 'bundle expected');
  assert.equal(await git(p.dir, 'show', 'HEAD:dist/index.html'), '<p>v2</p>');
});

test('underRoot: * matches one folder; prefix roots as before', async () => {
  const { underRoot } = await import('../src/workspace.ts');
  assert.ok(underRoot('programs/tip-jar/tests/tip_jar.rs', 'programs/*/tests'));
  assert.ok(!underRoot('programs/tip-jar/src/lib.rs', 'programs/*/tests'), 'код программы — не тесты');
  assert.ok(underRoot('tests/a.rs', 'tests'));
  assert.ok(underRoot('tests/a.rs', 'tests/'));
  assert.ok(!underRoot('testsx/a.rs', 'tests'));
  assert.ok(underRoot('programs', 'programs'));
});
