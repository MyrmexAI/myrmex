// Движок облачного агента: модель на лиз задаёт control plane в `assignment.model`.
// Модели Anthropic — Claude Code через OpenRouter (официально поддерживается); остальные — свой цикл агента.

import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { AssignmentPayload } from '@swarm/protocol';
import { ClaudeCodeRunner, type RunResult, type Runner } from './runner.ts';
import { ToolLoopRunner } from './toolloop.ts';
import type { DockerSandboxFactory } from './sandbox.ts';

export const DEFAULT_HOSTED_MODEL = 'anthropic/claude-sonnet-5.5';

export class HostedRunner implements Runner {
  runtime: 'hosted' | 'gateway';
  /** openRouterKey — облачный агент у нас; без ключа — своё устройство холдера, модели через шлюз из assignment.gateway. */
  constructor(private opts: { openRouterKey?: string | null; claudeBin?: string; claudeConfigDir?: string; sandbox?: DockerSandboxFactory | null }) {
    this.runtime = opts.openRouterKey ? 'hosted' : 'gateway';
  }

  /** Переменные окружения Claude Code для OpenRouter (ANTHROPIC_API_KEY обязательно пустой). */
  claudeEnv(key = this.opts.openRouterKey ?? '', base = 'https://openrouter.ai/api'): Record<string, string> {
    return {
      ANTHROPIC_BASE_URL: base,
      ANTHROPIC_AUTH_TOKEN: key,
      ANTHROPIC_API_KEY: '',
    };
  }

  async run(dir: string, a: AssignmentPayload, signal: AbortSignal): Promise<RunResult> {
    const model = a.model ?? DEFAULT_HOSTED_MODEL;
    const key = a.gateway?.token ?? this.opts.openRouterKey;
    if (!key) throw new Error('no model key: the task has no gateway token and the worker has no OPENROUTER_API_KEY');
    const base = a.gateway?.baseUrl ?? 'https://openrouter.ai/api';
    // песочница лиза (Docker): команды агента и Claude Code — внутри контейнера, сеть — по белому списку
    const sandbox = this.opts.sandbox ? await this.opts.sandbox.create(dir, a.leaseId) : undefined;
    try {
      const runner: Runner = model.startsWith('anthropic/')
        ? new ClaudeCodeRunner({ bin: this.opts.claudeBin, env: { ...this.claudeEnv(key, base), ANTHROPIC_SMALL_FAST_MODEL: model }, model, isolated: { configDir: this.opts.claudeConfigDir ?? join(tmpdir(), 'swarm-hosted-claude') }, sandbox })
        : new ToolLoopRunner({ apiKey: key, baseURL: `${base}/v1`, model, sandbox });
      return await runner.run(dir, a, signal);
    } finally {
      await sandbox?.close();
    }
  }
}
