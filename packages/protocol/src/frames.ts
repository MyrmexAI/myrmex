// Кадры WSS /agent. Каждый кадр — подписанный конверт:
// воркер подписывает ключом устройства, сервер — своим ключом сервиса.

export const WORKER_KINDS = ['hello', 'heartbeat', 'lease.accept', 'lease.decline', 'progress', 'submission', 'error'] as const;
export const SERVER_KINDS = ['challenge', 'welcome', 'assignment', 'ack', 'cancel', 'disconnect', 'error'] as const;
export type WorkerKind = (typeof WORKER_KINDS)[number];
export type ServerKind = (typeof SERVER_KINDS)[number];

/** Подписи загрузок: kind в signingPreimage. */
export const BUNDLE_KIND = 'bundle';
export const artifactKind = (leaseId: string) => `artifact:${leaseId}`;

export type Runtime = 'claude-code' | 'codex' | 'hosted' | 'gateway';

export type ChallengePayload = { v: 1; nonce: string; serverTime: string };

export type HelloPayload = {
  v: 1;
  deviceKey: string;
  nonce: string;
  version: string;
  runtimes: Runtime[];
  skills: string[];
  concurrency: number;
};

export type WelcomePayload = { v: 1; seatId: string; wallet: string; heartbeatSeconds: number; serverTime: string };
export type HeartbeatPayload = { v: 1; at: string; working: string[] };

export type FileRef = { name: string; path: string; hash: string; mediaType: string; bytes: number; url: string };
export type OutputSpec = { name: string; path: string; mediaType: string };

export type AssignmentPayload = {
  v: 1;
  leaseId: string;
  jobId: string;
  nodeId: string;
  skill: string;
  skillHash: string;
  role: 'implement' | 'review' | 'research' | 'media' | 'oracle';
  objective: string;
  acceptanceCriteria: string[];
  references: string[];
  /** Входы: файлы других принятых работ и исходный код (git bundle). */
  inputs: FileRef[];
  source: { repoUrl: string; baseCommit: string } | { bundleHash: string; baseCommit: string } | null;
  /** Файлы контекста, которые воркер кладёт в .swarm/reads/. */
  reads: { path: string; content: string }[];
  paths: string[];
  outputs: OutputSpec[];
  deadline: string;
  limits: { modelTokens: number; wallClockSeconds: number; diskMb: number };
  /** Модель для лиза (идентификатор OpenRouter). Облачный агент обязан её использовать; свой воркер — подсказка. */
  model: string | null;
  /** Модели генерации медиа (OpenRouter) для навыков create-image/audio/video; иначе null. */
  media?: MediaModels | null;
  /** Своё устройство холдера (runtime gateway): модели — через шлюз сервера `<baseUrl>` токеном лиза с бюджетом; ключ OpenRouter у холдера не бывает. */
  gateway?: { baseUrl: string; token: string; budgetUsd: number } | null;
};

export type MediaModels = { image: string; imageFallback?: string; vector: string; video: string; speech: string; voice?: string; music?: string };

export type LeaseReplyPayload = { v: 1; leaseId: string; reason?: string };
export type ProgressPayload = { v: 1; leaseId: string; stage: string; detail?: string };

export type Finding = { severity: 'critical' | 'high' | 'medium' | 'low' | 'info'; title: string; detail: string; location?: string };

export type SubmissionPayload = {
  v: 1;
  leaseId: string;
  jobId: string;
  nodeId: string;
  /** git bundle с изменениями (null — работа без кода). */
  bundleHash: string | null;
  headCommit: string | null;
  artifacts: { name: string; path: string; hash: string; mediaType: string; bytes: number }[];
  /** Для ревью: вердикт рецензента. */
  verdict: 'accept' | 'reject' | null;
  findings: Finding[];
  summary: string;
  usage: { runtime: Runtime; inputTokens: number; outputTokens: number; wallClockMs: number; model?: string };
};

export type AckPayload = { v: 1; ref: string; ok: boolean; detail?: string };
export type CancelPayload = { v: 1; leaseId: string; reason: string };
export type DisconnectPayload = { v: 1; reason: string };
export type ErrorPayload = { v: 1; error: string; detail?: string };
