// CLI воркера: pair | start | status | revoke.
//   SWARM_API=https://api.<домен> npx tsx src/cli.ts pair
//   npx tsx src/cli.ts start [--runtime gateway|claude-code|codex] [--concurrency 1]

import { seal } from '@swarm/protocol';
import { loadDeviceKey, loadWorkerConfig } from './config.ts';
import { Api, Daemon, pair } from './daemon.ts';
import { ClaudeCodeRunner, CodexRunner } from './runner.ts';
import { HostedRunner } from './hosted.ts';
import { DockerSandboxFactory } from './sandbox.ts';

const [cmd, ...rest] = process.argv.slice(2);
const flag = (name: string) => {
  const i = rest.indexOf(`--${name}`);
  return i >= 0 ? rest[i + 1] : undefined;
};

const cfg = loadWorkerConfig({
  ...(flag('api') ? { api: flag('api')! } : {}),
  ...(flag('runtime') ? { runtime: flag('runtime') as any } : {}),
  ...(flag('concurrency') ? { concurrency: Number(flag('concurrency')) } : {}),
});
const key = loadDeviceKey(cfg.home);
const api = new Api(cfg.api);
const log = (m: string) => console.log(m);

switch (cmd) {
  case 'pair':
    log(`Device key: ${key.publicKey}`);
    await pair(api, key, log);
    break;
  case 'start': {
    // full-режим (свой Claude Code / Codex) работает в отдельном каталоге без контейнера; compute — всегда в Docker-песочнице
    if (cfg.runtime === 'codex' || cfg.runtime === 'claude-code') log(`Runtime ${cfg.runtime}: your agent works in its own folder (${cfg.home}), outside Docker.`);
    log(`Starting the worker (${cfg.runtime === 'codex' || cfg.runtime === 'claude-code' ? 'full' : 'compute'} provider) → ${cfg.api}. Keep this window open; Ctrl+C stops it.`);
    // по умолчанию (gateway): компьютер холдера + Docker-песочница, модели — через шлюз нашего сервера (ключ не нужен)
    const runner =
      cfg.runtime === 'codex'
        ? new CodexRunner()
        : cfg.runtime === 'claude-code'
          ? new ClaudeCodeRunner()
          : new HostedRunner({ sandbox: new DockerSandboxFactory({ extraAllow: [new URL(cfg.api).hostname] }) });
    const d = new Daemon(cfg, key, runner);
    process.on('SIGINT', () => d.stop());
    await d.start();
    break;
  }
  case 'status':
    console.log(JSON.stringify(await api.json('GET', `/workers/${key.publicKey}/standing`), null, 2));
    break;
  case 'revoke': {
    const reason = flag('reason') ?? 'user_requested';
    const env = seal(key, 'enrollment.revoke', { v: 1, deviceKey: key.publicKey, reason, requestedAt: new Date().toISOString() });
    console.log(JSON.stringify(await api.json('POST', '/enrollments/revoke', env), null, 2));
    break;
  }
  default:
    log('Commands: pair | start [--runtime claude-code|codex] | status | revoke [--reason decommissioned|rotating|user_requested]');
    process.exitCode = 1;
}
