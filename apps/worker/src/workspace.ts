// Рабочий каталог лиза: исходники (git), контекст в .swarm/reads/, сбор результата (git bundle + артефакты).

import { execFile } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, normalize, relative, resolve, sep, extname } from 'node:path';
import { promisify } from 'node:util';
import { sha256Hex, type AssignmentPayload } from '@swarm/protocol';

const run = promisify(execFile);

export async function git(cwd: string, ...args: string[]) {
  // каталог лиза может принадлежать агенту песочницы (uid 1000), а воркер — root: без safe.directory git откажет
  const { stdout } = await run('git', ['-c', 'safe.directory=*', ...args], { cwd, maxBuffer: 64 << 20, env: { ...process.env, GIT_TERMINAL_PROMPT: '0' } });
  return stdout.trim();
}

const IDENTITY = ['-c', 'user.name=swarm-worker', '-c', 'user.email=worker@swarm.local', '-c', 'commit.gpgsign=false'];

// Файл внутри разрешённого корня; звёздочка — одна папка любого имени ("programs/<любая>/tests" — тесты каждой программы).
export function underRoot(file: string, root: string) {
  const parts = root.replace(/\/+$/, '').split('/');
  const segs = file.split('/');
  if (segs.length < parts.length) return false;
  return parts.every((p, i) => p === '*' || p === segs[i]);
}

/** Путь внутри каталога (без выхода через ..). */
export function safeJoin(root: string, rel: string) {
  const p = normalize(join(root, rel));
  if (p !== root && !p.startsWith(root + sep)) throw new Error(`path outside the workspace: ${rel}`);
  return p;
}

export type Prepared = { dir: string; baseCommit: string | null };

/** Артефакты сборки и тестов — исключаются из git status рабочего каталога. */
export const BUILD_JUNK = ['.swarm/', 'node_modules/', 'target/', 'dist/', 'build/', '__pycache__/', '*.pyc', '.pytest_cache/', '.venv/', 'coverage/', '.next/', 'test-ledger/'];

/** Навыки сайта: их статическая сборка (dist/ | out/ | build/) коммитится — её публикует доставка на IPFS. */
export const SITE_SKILLS = new Set(['build-website', 'frontend-for-program', 'import-site']);
const SITE_DIRS = ['dist', 'out', 'build'];

export async function prepare(root: string, a: AssignmentPayload, fetchBytes: (url: string) => Promise<Buffer>): Promise<Prepared> {
  // абсолютный путь: git и Docker работают из других каталогов, относительный путь там ломается
  const dir = resolve(root, a.leaseId);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  let baseCommit: string | null = null;
  if (a.source && 'repoUrl' in a.source) {
    await run('git', ['clone', '--quiet', a.source.repoUrl, dir], { maxBuffer: 64 << 20 });
    await git(dir, 'checkout', '--quiet', a.source.baseCommit);
    baseCommit = a.source.baseCommit;
  } else if (a.source && 'bundleHash' in a.source) {
    const bundle = join(root, `${a.leaseId}.bundle`);
    writeFileSync(bundle, await fetchBytes(a.source.bundleHash));
    await run('git', ['clone', '--quiet', bundle, dir]);
    baseCommit = await git(dir, 'rev-parse', 'HEAD');
  } else {
    await git(dir, 'init', '--quiet');
    writeFileSync(join(dir, '.gitignore'), '.swarm/\nnode_modules/\ntarget/\n');
    await git(dir, ...IDENTITY, 'add', '.gitignore');
    await git(dir, ...IDENTITY, 'commit', '--quiet', '-m', 'swarm: empty workspace');
    baseCommit = await git(dir, 'rev-parse', 'HEAD');
  }
  // контекст и входы — в .swarm/reads (не коммитятся)
  for (const r of a.reads) {
    const p = safeJoin(dir, r.path);
    mkdirSync(dirname(p), { recursive: true });
    writeFileSync(p, r.content);
  }
  for (const f of a.inputs) {
    const bytes = await fetchBytes(f.url);
    if (f.bytes && sha256Hex(bytes) !== f.hash) throw new Error(`input ${f.name}: hash mismatch`);
    const p = safeJoin(dir, f.path.startsWith('.swarm/') ? f.path : join('.swarm/reads/artifacts', extname(f.name) ? f.name : f.name + extname(f.path)));
    mkdirSync(dirname(p), { recursive: true });
    writeFileSync(p, bytes);
  }
  mkdirSync(join(dir, '.swarm'), { recursive: true });
  // служебные файлы сборки и тестов не считаются изменениями работы (действует и для чужих репозиториев)
  mkdirSync(join(dir, '.git', 'info'), { recursive: true });
  writeFileSync(join(dir, '.git', 'info', 'exclude'), BUILD_JUNK.join('\n') + '\n');
  return { dir, baseCommit };
}

export type Collected = {
  bundle: Buffer | null;
  headCommit: string | null;
  artifacts: { name: string; path: string; mediaType: string; bytes: Buffer }[];
  result: { summary: string; verdict: 'accept' | 'reject' | null; findings: any[] };
  changed: string[];
};

/** Собрать результат и проверить, что изменены только разрешённые пути. */
export async function collect(p: Prepared, a: AssignmentPayload): Promise<Collected> {
  const resultFile = join(p.dir, '.swarm', 'result.json');
  const result = existsSync(resultFile) ? JSON.parse(readFileSync(resultFile, 'utf8')) : {};
  const artifacts = a.outputs.map((o) => {
    const file = safeJoin(p.dir, o.path);
    if (!existsSync(file)) throw new Error(`missing declared output ${o.name} (${o.path})`);
    return { name: o.name, path: o.path, mediaType: o.mediaType, bytes: readFileSync(file) };
  });

  const status = await git(p.dir, 'status', '--porcelain', '--untracked-files=all');
  const changed = status
    .split('\n')
    .filter(Boolean)
    .map((l) => l.replace(/^\s*[MADRCU?!T]{1,2}\s+/, '').replace(/^.* -> /, '').replace(/^"|"$/g, ''))
    .filter((f) => !f.startsWith('.swarm/') && !a.outputs.some((o) => o.path === f));
  // агент мог сам закоммитить свои изменения: тогда рабочее дерево чистое, но HEAD ушёл от стартового коммита
  const committed = p.baseCommit && (await git(p.dir, 'rev-parse', 'HEAD')) !== p.baseCommit
    ? (await git(p.dir, 'diff', '--name-only', p.baseCommit, 'HEAD')).split('\n').filter((f) => f && !f.startsWith('.swarm/') && !a.outputs.some((o) => o.path === f))
    : [];
  const touched = [...new Set([...changed, ...committed])];
  const allowed = a.paths;
  const outside = a.role === 'review' ? touched : touched.filter((f) => !allowed.some((root) => underRoot(f, root)));
  if (allowed.length === 0 && a.role !== 'review') {
    // навыки без своего бюджета записи пишут куда угодно, кроме .swarm
  } else if (outside.length) {
    throw new Error(`changed paths outside the allowed ones: ${outside.slice(0, 5).join(', ')}`);
  }

  let bundle: Buffer | null = null;
  let headCommit: string | null = null;
  const siteDist = a.role !== 'review' && SITE_SKILLS.has(a.skill) ? SITE_DIRS.find((d) => existsSync(join(p.dir, d, 'index.html'))) : undefined;
  if ((touched.length || siteDist) && a.role !== 'review') {
    // -f: путь уже показан git status (отслеживаемый файл), но может попадать под исключения сборки — например,
    // dist/ сайта из репозитория заказчика; без -f git откажет «paths are ignored»
    for (const f of changed) await git(p.dir, ...IDENTITY, 'add', '-f', '--', f);
    if (siteDist) await git(p.dir, ...IDENTITY, 'add', '-f', '--', siteDist);
    // коммит — только если есть что коммитить (агент мог закоммитить всё сам)
    if ((await git(p.dir, 'status', '--porcelain')).split('\n').some((l) => l && !l.slice(3).startsWith('.swarm/')))
      await git(p.dir, ...IDENTITY, 'commit', '--quiet', '-m', `swarm: ${a.skill} (${a.nodeId.slice(0, 8)})`);
    headCommit = await git(p.dir, 'rev-parse', 'HEAD');
    const file = join(p.dir, '.swarm', 'changes.bundle');
    await git(p.dir, 'bundle', 'create', file, 'HEAD', '--all');
    bundle = readFileSync(file);
  }
  return {
    bundle,
    headCommit,
    artifacts,
    result: {
      summary: String(result.summary ?? '').slice(0, 4_000) || `${a.skill}: done`,
      verdict: result.verdict === 'accept' || result.verdict === 'reject' ? result.verdict : a.role === 'review' ? 'reject' : null,
      findings: Array.isArray(result.findings) ? result.findings.slice(0, 50) : [],
    },
    changed: [...changed.map((f) => relative('.', f)), ...(siteDist ? [`${siteDist}/`] : [])],
  };
}
