// Сообщения, которые подписывает кошелёк Solana (signMessage, Ed25519) — аналог EIP-712 для Solana.
// Текст: "<PREFIX>\n<TYPE>\n<canonicalJson(message)>" — читается в окне кошелька и однозначно разбирается.

import { ed25519 } from '@noble/curves/ed25519.js';
import { addressToBytes, base58Decode, utf8 } from './bytes.ts';
import { canonicalJson } from './canonical.ts';
import { PROTOCOL_PREFIX } from './envelope.ts';

export type WorkerAuthorization = {
  v: 1;
  deviceKey: string; // 64 hex
  wallet: string; // base58
  seatId: string; // адрес PDA места, base58
  nonce: string; // 64 hex из /pair/start
  expiresAt: number; // Unix секунды
  relayOrigin: string;
};

export type QuoteApproval = {
  resource: string;
  requesterScopeHash: string;
  quoteId: string;
  quoteHash: string;
  paymentHash: string;
  action: string;
  mint: string;
  amount: string; // атомарные единицы, десятичная строка
  payTo: string;
  expiresAt: number;
};

/** Вход в админку сайта: подпись кошелька админа (ключей на сервере нет). */
export type AdminSession = {
  v: 1;
  wallet: string;
  nonce: string; // 64 hex из /admin/session/start
  origin: string;
  expiresAt: number;
};

/** Действие владельца расписания (отмена с возвратом неиспользованных запусков). */
export type ScheduleAction = { v: 1; scheduleId: string; action: 'cancel'; wallet: string; expiresAt: number };

/**
 * Запрос выплаты начислений в SOL (токеномика 2026-10-08): провайдер или заказчик запуска подписывает его на сайте,
 * админ отправляет выплату из админки. accruals — id начислений или 'all' (все доступные этого вида).
 */
export type ClaimRequest = { v: 1; wallet: string; kind: 'orders' | 'bonus' | 'launches'; accruals: number[] | 'all'; nonce: string; expiresAt: number };

export type WalletMessageType = 'WorkerAuthorization' | 'QuoteApproval' | 'SeatOwnerAction' | 'AdminSession' | 'ScheduleAction' | 'ClaimRequest';

export const walletMessageText = (type: WalletMessageType, message: unknown): string =>
  `${PROTOCOL_PREFIX}\n${type}\n${canonicalJson(message)}`;

export const walletMessageBytes = (type: WalletMessageType, message: unknown): Uint8Array =>
  utf8(walletMessageText(type, message));

/** signature — base58 (как отдают кошельки Solana) или hex 128 символов. */
export function verifyWalletMessage(wallet: string, type: WalletMessageType, message: unknown, signature: string): boolean {
  try {
    const sig = /^[0-9a-f]{128}$/.test(signature)
      ? Uint8Array.from(signature.match(/../g)!.map((h) => parseInt(h, 16)))
      : base58Decode(signature);
    if (sig.length !== 64) return false;
    return ed25519.verify(sig, walletMessageBytes(type, message), addressToBytes(wallet));
  } catch {
    return false;
  }
}
