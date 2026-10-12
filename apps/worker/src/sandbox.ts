// Docker-песочница лиза: контейнер на каждый лиз — только рабочий каталог (/work),
// непривилегированный пользователь, без capabilities, лимиты CPU/RAM/процессов; сеть — внутренняя сеть Docker
// без выхода наружу, выход только через прокси с белым списком доменов (infra/sandbox/proxy/allowlist).
// В песочнице выполняются команды агента (свой цикл) и сам Claude Code; ключи хоста туда не попадают —
// передаётся только то, что нужно агенту (ключ модели), и только в процесс агента.

import { resolve } from 'node:path';
import { spawn } from 'node:child_process';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const run = promisify(execFile);

export type ExecResult = { code: number; stdout: string; stderr: string };

export interface Sandbox {
  readonly name: string;
  /** Выполнить argv в контейнере (рабочий каталог /work). */
  exec(argv: string[], opts: { timeoutMs: number; signal?: AbortSignal; env?: Record<string, string>; input?: string }): Promise<ExecResult>;
  close(): Promise<void>;
}

export type SandboxOptions = {
  image?: string;
  proxyImage?: string;
  /** Внутренняя сеть песочниц (без выхода наружу). */
  network?: string;
  proxyName?: string;
  /** Доп. домены белого списка прокси (у холдера — хост нашего API со шлюзом моделей). */
  extraAllow?: string[];
  cpus?: number;
  memory?: string;
  pids?: number;
};

const docker = (args: string[], timeout = 120_000) =>
  run('docker', args, { maxBuffer: 16 << 20, timeout }).catch((e) => {
    throw new Error(`docker ${args[0]}: ${String(e.stderr || e.message).slice(-800)}`);
  });

export class DockerSandboxFactory {
  private ready: Promise<void> | null = null;
  readonly opts: Required<SandboxOptions>;

  constructor(opts: SandboxOptions = {}) {
    this.opts = {
      image: opts.image ?? 'swarm-agent-sandbox:1',
      proxyImage: opts.proxyImage ?? 'swarm-sandbox-proxy:1',
      network: opts.network ?? 'swarm-sandbox',
      proxyName: opts.proxyName ?? 'swarm-sandbox-proxy',
      cpus: opts.cpus ?? 2,
      memory: opts.memory ?? '4g',
      pids: opts.pids ?? 512,
      extraAllow: opts.extraAllow ?? [],
    };
  }

  /** Внутренняя сеть и прокси выхода (один на машину); идемпотентно. */
  ensureNetwork(): Promise<void> {
    return (this.ready ??= (async () => {
      const { network, proxyName, proxyImage } = this.opts;
      const nets = (await docker(['network', 'ls', '--format', '{{.Name}}'])).stdout.split('\n');
      if (!nets.includes(network)) await docker(['network', 'create', '--internal', network]);
      const running = (await docker(['ps', '-a', '--filter', `name=^${proxyName}$`, '--format', '{{.State}}'])).stdout.trim();
      if (!running) {
        await docker(['run', '-d', '--name', proxyName, '--restart', 'unless-stopped', '-e', `EXTRA_ALLOW=${(this.opts.extraAllow ?? []).join(',')}`, proxyImage]);
        await docker(['network', 'connect', network, proxyName]);
      } else if (running !== 'running') await docker(['start', proxyName]);
    })().catch((e) => {
      this.ready = null;
      throw e;
    }));
  }

  async create(dir: string, leaseId: string): Promise<Sandbox> {
    await this.ensureNetwork();
    const name = `swarm-lease-${leaseId.replace(/[^a-zA-Z0-9-]/g, '').slice(0, 36)}`;
    const proxy = `http://${this.opts.proxyName}:8888`;
    await docker(['rm', '-f', name]).catch(() => {});
    // агент в образе — uid 1000; воркер под root создаёт каталог лиза от root → отдать его агенту, иначе /work только для чтения
    if (process.getuid?.() === 0) await run('chown', ['-R', '1000:1000', resolve(dir)]);
    await docker([
      'run', '-d', '--name', name,
      '--network', this.opts.network,
      '--cpus', String(this.opts.cpus), '--memory', this.opts.memory, '--pids-limit', String(this.opts.pids),
      '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
      '-e', `HTTP_PROXY=${proxy}`, '-e', `HTTPS_PROXY=${proxy}`, '-e', `http_proxy=${proxy}`, '-e', `https_proxy=${proxy}`, '-e', 'NO_PROXY=localhost,127.0.0.1',
      '-v', `${resolve(dir)}:/work`, '-w', '/work',
      // общий кэш скачанных крейтов между лизами (только загрузки; сборка у каждого лиза своя)
      '-v', 'swarm-cargo-registry:/home/agent/.cargo/registry', '-v', 'swarm-cargo-git:/home/agent/.cargo/git',
      this.opts.image, 'sleep', 'infinity',
    ]);
    return new DockerSandbox(name);
  }
}

class DockerSandbox implements Sandbox {
  constructor(readonly name: string) {}

  exec(argv: string[], opts: { timeoutMs: number; signal?: AbortSignal; env?: Record<string, string>; input?: string }): Promise<ExecResult> {
    const env = Object.entries(opts.env ?? {}).flatMap(([k, v]) => ['-e', `${k}=${v}`]);
    return new Promise((resolve, reject) => {
      const child = spawn('docker', ['exec', ...(opts.input !== undefined ? ['-i'] : []), ...env, this.name, ...argv], {
        signal: opts.signal,
        stdio: [opts.input === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'],
      });
      if (opts.input !== undefined) child.stdin!.end(opts.input);
      let stdout = '';
      let stderr = '';
      child.stdout!.on('data', (d) => (stdout += d));
      child.stderr!.on('data', (d) => (stderr += d));
      const t = setTimeout(() => child.kill('SIGKILL'), opts.timeoutMs);
      child.on('error', (e) => {
        clearTimeout(t);
        reject(e);
      });
      child.on('close', (code) => {
        clearTimeout(t);
        resolve({ code: code ?? -1, stdout, stderr });
      });
    });
  }

  async close() {
    await docker(['rm', '-f', this.name]).catch(() => {});
  }
}
