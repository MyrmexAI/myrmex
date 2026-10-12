// Воркер-демон: сопряжение, WSS, лизы. Один экземпляр — одно устройство — одно место.

import { join } from 'node:path';
import WebSocket from 'ws';
import {
  artifactKind,
  BUNDLE_KIND,
  open,
  seal,
  sha256Hex,
  signPreimage,
  type AssignmentPayload,
  type DeviceKeypair,
  type SubmissionPayload,
} from '@swarm/protocol';
import { WORKER_VERSION, type WorkerConfig } from './config.ts';
import type { Runner } from './runner.ts';
import { collect, prepare } from './workspace.ts';

export type Log = (msg: string, extra?: Record<string, unknown>) => void;

export class Api {
  constructor(readonly base: string) {}
  async json(method: string, path: string, body?: unknown) {
    const res = await fetch(this.base + path, {
      method,
      headers: body === undefined ? {} : { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const data = await res.json().catch(() => null);
    if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${JSON.stringify(data)}`);
    return data;
  }
  async bytes(urlOrPath: string) {
    const url = urlOrPath.startsWith('http') ? urlOrPath : this.base + urlOrPath;
    const res = await fetch(url);
    if (!res.ok) throw new Error(`GET ${url}: ${res.status}`);
    return Buffer.from(await res.arrayBuffer());
  }
  async upload(kind: 'bundle' | 'artifact', key: DeviceKeypair, bytes: Buffer, leaseId?: string) {
    const hash = sha256Hex(bytes);
    const headers: Record<string, string> = {
      'content-type': 'application/octet-stream',
      'x-swarm-device': key.publicKey,
      'x-swarm-signature': signPreimage(key.secretKey, kind === 'bundle' ? BUNDLE_KIND : artifactKind(leaseId!), hash),
      [kind === 'bundle' ? 'x-swarm-bundle-hash' : 'x-swarm-artifact-hash']: hash,
    };
    if (leaseId) headers['x-swarm-lease'] = leaseId;
    const res = await fetch(`${this.base}/${kind === 'bundle' ? 'bundles' : 'artifacts'}`, { method: 'POST', headers, body: new Uint8Array(bytes) });
    if (!res.ok) throw new Error(`upload ${kind}: ${res.status} ${await res.text()}`);
    return hash;
  }
}

/** Сопряжение: показать код и ссылку, ждать, пока холдер подпишет авторизацию кошельком. */
export async function pair(api: Api, key: DeviceKeypair, log: Log, pollMs = 2_000) {
  const start = await api.json('POST', '/pair/start', { deviceKey: key.publicKey });
  log(`Pairing code: ${start.code}`);
  log(`Open ${start.pairUrl}, connect the wallet that owns the seat, press "Pair here" and sign. The code is valid for 5 minutes. Waiting…`);
  for (;;) {
    const s = await api.json('GET', `/pair/${start.code}`);
    if (s.status === 'enrolled') {
      log(`Paired: this device is bound to seat ${s.seatId} (wallet ${s.wallet}). Now run: npx tsx apps/worker/src/cli.ts start`);
      return s;
    }
    if (s.status === 'expired') throw new Error('the code expired — run pair again');
    await new Promise((r) => setTimeout(r, pollMs));
  }
}

export class Daemon {
  private ws: WebSocket | null = null;
  private heartbeat: NodeJS.Timeout | null = null;
  private active = new Map<string, AbortController>();
  private stopped = false;
  readonly api: Api;

  constructor(
    readonly cfg: WorkerConfig,
    readonly key: DeviceKeypair,
    readonly runner: Runner,
    readonly log: Log = (m, e) => console.log(`[worker] ${m}`, e ?? ''),
  ) {
    this.api = new Api(cfg.api);
  }

  private send(kind: string, payload: unknown) {
    this.ws?.send(JSON.stringify(seal(this.key, kind, payload)));
  }

  /** Подключиться и держать соединение; при обрыве — переподключение с паузой. */
  async start(): Promise<void> {
    while (!this.stopped) {
      try {
        await this.connectOnce();
      } catch (e) {
        this.log(`connection: ${String(e)}`);
      }
      if (!this.stopped) await new Promise((r) => setTimeout(r, 3_000));
    }
  }

  stop() {
    this.stopped = true;
    for (const c of this.active.values()) c.abort();
    this.ws?.close();
  }

  connectOnce(): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.cfg.api.replace(/^http/, 'ws') + '/agent');
      this.ws = ws;
      let welcomed = false;
      ws.on('message', (raw) => {
        const r = open(JSON.parse(raw.toString()));
        if (!r.ok) return this.log(`server frame rejected: ${r.error}`);
        const env = r.envelope as any;
        switch (env.kind) {
          case 'challenge':
            this.send('hello', {
              v: 1,
              deviceKey: this.key.publicKey,
              nonce: env.payload.nonce,
              version: WORKER_VERSION,
              runtimes: [this.runner.runtime],
              skills: ['*'],
              concurrency: this.cfg.concurrency,
            });
            return;
          case 'welcome':
            welcomed = true;
            this.log(`online in the swarm: seat ${env.payload.seatId} — waiting for work`);
            this.heartbeat = setInterval(
              () => this.send('heartbeat', { v: 1, at: new Date().toISOString(), working: [...this.active.keys()] }),
              env.payload.heartbeatSeconds * 1000,
            );
            return;
          case 'assignment':
            void this.handle(env.payload as AssignmentPayload);
            return;
          case 'cancel':
            this.active.get(env.payload.leaseId)?.abort();
            return;
          case 'error':
            this.log(`server error: ${env.payload.error}`, env.payload);
            return;
          case 'disconnect':
            this.log(`disconnected by the server: ${env.payload.reason}`);
            if (['not_enrolled', 'revoked', 'worker_outdated', 'seat_inactive'].includes(env.payload.reason)) this.stopped = true;
            return;
        }
      });
      ws.on('close', () => {
        if (this.heartbeat) clearInterval(this.heartbeat);
        welcomed ? resolve() : reject(new Error('connection closed before welcome'));
      });
      ws.on('error', reject);
    });
  }

  /** Выполнить лиз целиком: принять → подготовить → агент → собрать → загрузить → сдать. */
  async handle(a: AssignmentPayload) {
    if (this.active.size >= this.cfg.concurrency) {
      this.send('lease.decline', { v: 1, leaseId: a.leaseId, reason: 'at_capacity' });
      return;
    }
    const ctrl = new AbortController();
    this.active.set(a.leaseId, ctrl);
    this.send('lease.accept', { v: 1, leaseId: a.leaseId });
    this.log(`task ${a.leaseId.slice(0, 8)}: ${a.skill}`);
    try {
      const prepared = await prepare(join(this.cfg.home, 'leases'), a, (u) => this.api.bytes(u.length === 64 ? `/bundles/${u}` : u));
      this.send('progress', { v: 1, leaseId: a.leaseId, stage: 'running' });
      const usage = await this.runner.run(prepared.dir, a, ctrl.signal);
      const out = await collect(prepared, a);
      const bundleHash = out.bundle ? await this.api.upload('bundle', this.key, out.bundle) : null;
      const artifacts = [];
      for (const f of out.artifacts) {
        const hash = await this.api.upload('artifact', this.key, f.bytes, a.leaseId);
        artifacts.push({ name: f.name, path: f.path, hash, mediaType: f.mediaType, bytes: f.bytes.length });
      }
      const sub: SubmissionPayload = {
        v: 1,
        leaseId: a.leaseId,
        jobId: a.jobId,
        nodeId: a.nodeId,
        bundleHash,
        headCommit: out.headCommit,
        artifacts,
        verdict: a.role === 'review' ? out.result.verdict : null,
        findings: out.result.findings,
        summary: out.result.summary,
        usage: {
          runtime: this.runner.runtime === 'script' ? 'claude-code' : this.runner.runtime,
          inputTokens: usage.inputTokens,
          outputTokens: usage.outputTokens,
          wallClockMs: usage.wallClockMs,
          // модель, которая реально работала (облачный агент — из assignment.model); по ней выбирается рецензент
          ...(usage.model ?? a.model ? { model: usage.model ?? a.model! } : {}),
        },
      };
      this.send('submission', sub);
      this.log(`task ${a.leaseId.slice(0, 8)}: submitted${sub.verdict ? ` (${sub.verdict}, ${sub.findings.length} findings)` : ''} — ${String(sub.summary).slice(0, 160)}`);
      if (a.role === 'review' && usage.log) this.log(`task ${a.leaseId.slice(0, 8)}: agent log tail: ${usage.log.slice(-600).replace(/\s+/g, ' ')}`);
    } catch (e) {
      this.log(`task ${a.leaseId.slice(0, 8)}: error ${String(e)}`);
      this.send('error', { v: 1, error: 'lease_failed', detail: String(e).slice(0, 500) });
      this.send('lease.decline', { v: 1, leaseId: a.leaseId, reason: 'failed' });
    } finally {
      this.active.delete(a.leaseId);
    }
  }
}
