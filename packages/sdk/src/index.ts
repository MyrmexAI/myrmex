// Клиент API роя для заказчиков: людей, проектов и других ИИ-агентов.
// Весь путь заказа: плитка/проверка → котировка → транзакция оплаты от сервера → подпись кошельком →
// submit → статус работы и результат. Кошелёк — любой, кто умеет подписать транзакцию и сообщение.
//
//   const swarm = new SwarmClient({ api: 'https://api.<домен>' });
//   const { action, input } = await swarm.compose('image', { description: 'Лис в худи, аватар' });
//   const order = await swarm.order(action, input, wallet);   // котировка + оплата + допуск
//   const job = await swarm.waitJob(order.jobId);

// Web Crypto: работает и в Node 24, и в браузере
import {
  createKeyPairSignerFromBytes,
  getBase64EncodedWireTransaction,
  getBase64Encoder,
  getTransactionDecoder,
  partiallySignTransaction,
  type Transaction,
} from '@solana/kit';
import { base58Encode, sha256Hex, walletMessageBytes, type QuoteApproval } from '@swarm/protocol';

/** Кошелёк заказчика: подпись транзакции (base64 → base64) и сообщения (байты → 64 байта подписи). */
export interface SwarmWallet {
  address: string;
  signTransaction(transactionBase64: string): Promise<string>;
  signMessage(message: Uint8Array): Promise<Uint8Array>;
}

/** Кошелёк из секретного ключа (64 байта, как solana-keygen) — для серверов и ИИ-агентов. */
export async function keypairWallet(secretKey: Uint8Array): Promise<SwarmWallet> {
  const signer = await createKeyPairSignerFromBytes(secretKey);
  return {
    address: signer.address,
    async signTransaction(b64) {
      const tx = getTransactionDecoder().decode(getBase64Encoder().encode(b64)) as Transaction;
      return getBase64EncodedWireTransaction(await partiallySignTransaction([signer.keyPair], tx as any));
    },
    async signMessage(message) {
      const [sig] = await signer.signMessages([{ content: message, signatures: {} }]);
      return sig![signer.address]!;
    },
  };
}

export class SwarmApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    readonly detail: unknown,
  ) {
    super(`${status} ${code}${detail ? `: ${typeof detail === 'string' ? detail : JSON.stringify(detail)}` : ''}`);
  }
}

export type Order = { orderId: string; status: string; jobId: string | null; admission: any; payment: any };

export class SwarmClient {
  readonly token: string;
  constructor(private opts: { api: string; token?: string; fetchImpl?: typeof fetch }) {
    // токен запроса: 32 случайных байта hex — область идемпотентности котировок
    this.token = opts.token ?? Array.from(crypto.getRandomValues(new Uint8Array(32)), (b) => b.toString(16).padStart(2, '0')).join('');
  }

  private async call<T = any>(method: string, path: string, body?: unknown, auth = false): Promise<{ status: number; body: T }> {
    const res = await (this.opts.fetchImpl ?? fetch)(`${this.opts.api}${path}`, {
      method,
      headers: { ...(body !== undefined ? { 'content-type': 'application/json' } : {}), ...(auth ? { authorization: `Bearer ${this.token}` } : {}) },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    const data = (await res.json().catch(() => null)) as any;
    if (res.status >= 400 && res.status !== 402) throw new SwarmApiError(res.status, data?.error ?? 'error', data?.detail ?? data?.problems);
    return { status: res.status, body: data as T };
  }

  capabilities() {
    return this.call('GET', '/requests/capabilities').then((r) => r.body);
  }
  tiles() {
    return this.call('GET', '/requests/tiles').then((r) => r.body.tiles);
  }
  /** Форма плитки → действие, вход, план с навыками и блокеры (ничего не списывается). */
  compose(tile: string, form: Record<string, unknown>) {
    return this.call('POST', '/requests/compose', { tile, form }).then((r) => r.body);
  }
  check(action: string, input: unknown) {
    return this.call('POST', '/requests/check', { action, input }).then((r) => r.body);
  }
  importRepo(url: string, kind: 'site' | 'programs' | 'code' = 'code', ref?: string) {
    return this.call('POST', '/requests/import', { url, kind, ref }).then((r) => r.body);
  }
  quote(action: string, input: unknown, requestKey: string = crypto.randomUUID()) {
    return this.call('POST', '/requests/quote', { requestKey, action, input }, true).then((r) => r.body.order);
  }
  status(orderId: string) {
    return this.call('GET', `/requests/${orderId}`, undefined, true).then((r) => r.body);
  }
  job(jobId: string) {
    return this.call('GET', `/jobs/${jobId}`).then((r) => r.body);
  }
  result(jobId: string) {
    return this.call('GET', `/jobs/${jobId}/result`).then((r) => r.body);
  }

  /** Оплатить котировку: транзакция от сервера → подпись кошельком + QuoteApproval → submit. */
  async pay(orderId: string, wallet: SwarmWallet): Promise<Order> {
    const terms = await this.call('POST', `/requests/${orderId}/submit`, undefined, true);
    if (terms.status !== 402) return this.orderView(orderId, terms.body);
    const ptx = (await this.call('POST', `/requests/${orderId}/payment-tx`, { payer: wallet.address }, true)).body;
    // бесплатный заказ (запуск токена): перевода нет — только подпись QuoteApproval
    const transaction = ptx.free ? undefined : await wallet.signTransaction(ptx.transaction);
    const q = terms.body.quote;
    const approval: QuoteApproval = {
      resource: terms.body.resourceUrl,
      requesterScopeHash: sha256Hex(this.token),
      quoteId: orderId,
      quoteHash: q.quoteHash,
      paymentHash: ptx.paymentHash,
      action: q.action,
      mint: q.payment.asset,
      amount: q.payment.amount,
      payTo: q.payment.payTo,
      expiresAt: q.expiresAt,
    };
    const quoteSignature = base58Encode(await wallet.signMessage(walletMessageBytes('QuoteApproval', approval)));
    const done = await this.call('POST', `/requests/${orderId}/submit`, { payer: wallet.address, paymentHash: ptx.paymentHash, quoteSignature, transaction }, true);
    return this.orderView(orderId, done.body);
  }

  /** Котировка + оплата одним вызовом. */
  async order(action: string, input: unknown, wallet: SwarmWallet, requestKey?: string): Promise<Order> {
    const quoted = await this.quote(action, input, requestKey);
    return this.pay(quoted.id, wallet);
  }

  /** Ждать конца работы (completed | blocked | failed | cancelled). */
  async waitJob(jobId: string, opts: { timeoutMs?: number; intervalMs?: number } = {}) {
    const end = Date.now() + (opts.timeoutMs ?? 30 * 60_000);
    while (Date.now() < end) {
      const j = await this.job(jobId);
      if (['completed', 'blocked', 'failed', 'cancelled'].includes(j.state)) return j;
      await new Promise((r) => setTimeout(r, opts.intervalMs ?? 5_000));
    }
    throw new Error(`работа ${jobId} не закончилась за отведённое время`);
  }

  private orderView(orderId: string, b: any): Order {
    return { orderId, status: b.status, jobId: b.admission?.result?.jobId ?? null, admission: b.admission ?? null, payment: b.payment ?? null };
  }
}
