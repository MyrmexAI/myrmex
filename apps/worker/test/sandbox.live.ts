// Живая проверка Docker-песочницы лиза (нужен Docker и образы из infra/sandbox):
//   docker build -t swarm-sandbox-proxy:1 infra/sandbox/proxy && docker build -t swarm-agent-sandbox:1 infra/sandbox
//   npx tsx apps/worker/test/sandbox.live.ts
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DockerSandboxFactory } from '../src/sandbox.ts';

const dir = mkdtempSync(join(tmpdir(), 'sbx-'));
const sb = await new DockerSandboxFactory().create(dir, `check-${Date.now()}`);
const sh = (cmd: string) => sb.exec(['sh', '-lc', cmd], { timeoutMs: 60_000 });
let ok = 0;
let bad = 0;
const check = (name: string, pass: boolean, detail = '') => {
  console.log(`${pass ? '✔' : '✖'} ${name}${detail ? ` — ${detail}` : ''}`);
  pass ? ok++ : bad++;
};
try {
  const uid = (await sh('id -u')).stdout.trim();
  check('не root', uid === '1000', `uid ${uid}`);
  const tools = (await sh('cargo --version && cargo build-sbf --version | head -1 && node --version && python3 --version && claude --version')).stdout.trim().split('\n');
  check('инструменты: Rust, Solana, Node, Python, Claude Code', tools.length >= 5, tools.join(' | '));
  const allowed = (await sh('curl -s -o /dev/null -w "%{http_code}" https://index.crates.io/config.json')).stdout.trim();
  check('белый список: crates.io через прокси', allowed === '200', `HTTP ${allowed}`);
  const npm = (await sh('curl -s -o /dev/null -w "%{http_code}" https://registry.npmjs.org/react')).stdout.trim();
  check('белый список: npm через прокси', npm === '200', `HTTP ${npm}`);
  const denied = await sh('curl -s -o /dev/null -w "%{http_code}" --max-time 15 https://example.com');
  check('вне списка (example.com) — отказ', denied.stdout.trim() !== '200', `HTTP ${denied.stdout.trim() || denied.code}`);
  const direct = await sh('curl -s -o /dev/null -w "%{http_code}" --max-time 10 --noproxy "*" https://index.crates.io/config.json');
  check('прямой выход мимо прокси — нет', direct.stdout.trim() !== '200', `код ${direct.code}, HTTP ${direct.stdout.trim() || '—'}`);
  const mem = (await sh('cat /sys/fs/cgroup/memory.max')).stdout.trim();
  check('лимит памяти', mem !== 'max' && Number(mem) <= 4 * 1024 ** 3, mem);
  await sh('echo hello > /work/from-sandbox.txt');
  check('рабочий каталог общий с хостом', existsSync(join(dir, 'from-sandbox.txt')) && readFileSync(join(dir, 'from-sandbox.txt'), 'utf8').trim() === 'hello');
  const host = (await sh('ls /mnt /host 2>/dev/null | head -3; ls /work')).stdout.trim();
  check('файлы хоста не видны (только /work)', !/Users|Windows/.test(host), host.replace(/\n/g, ' '));
} finally {
  await sb.close();
  rmSync(dir, { recursive: true, force: true });
}
console.log(`\n${ok} ok, ${bad} fail`);
process.exitCode = bad ? 1 : 0;
