// Локальное состояние воркера: ключ устройства и рабочие каталоги лизов.
// Ключ устройства — НЕ кошелёк: он только подписывает кадры и загрузки этого устройства.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { deviceKeyFromSecret, fromHex, generateDeviceKey, toHex, type DeviceKeypair } from '@swarm/protocol';

export const WORKER_VERSION = '0.1.0';

export type WorkerConfig = {
  home: string;
  api: string;
  concurrency: number;
  runtime: 'gateway' | 'claude-code' | 'codex' | 'script';
  sandbox: 'local' | 'docker';
  dockerImage: string;
};

export function loadWorkerConfig(over: Partial<WorkerConfig> = {}): WorkerConfig {
  return {
    home: process.env.SWARM_WORKER_HOME ?? join(homedir(), '.swarm-worker'),
    api: process.env.SWARM_API ?? 'http://localhost:8787',
    concurrency: Number(process.env.SWARM_CONCURRENCY ?? 1),
    runtime: (process.env.SWARM_RUNTIME as WorkerConfig['runtime']) ?? 'gateway',
    sandbox: (process.env.SWARM_SANDBOX as WorkerConfig['sandbox']) ?? 'local',
    dockerImage: process.env.SWARM_DOCKER_IMAGE ?? 'swarm/worker-sandbox:latest',
    ...over,
  };
}

/** Ключ устройства: создаётся при первом запуске, хранится в <home>/device.json. */
export function loadDeviceKey(home: string): DeviceKeypair {
  mkdirSync(home, { recursive: true });
  const file = join(home, 'device.json');
  if (existsSync(file)) {
    const { secretKey } = JSON.parse(readFileSync(file, 'utf8')) as { secretKey: string };
    return deviceKeyFromSecret(fromHex(secretKey));
  }
  const key = generateDeviceKey();
  writeFileSync(file, JSON.stringify({ publicKey: key.publicKey, secretKey: toHex(key.secretKey) }, null, 2), { mode: 0o600 });
  return key;
}
