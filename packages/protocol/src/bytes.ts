import { sha256 as nobleSha256 } from '@noble/hashes/sha2.js';
import { canonicalJson } from './canonical.ts';

const enc = new TextEncoder();

export const utf8 = (s: string): Uint8Array => enc.encode(s);

export function toHex(bytes: Uint8Array): string {
  let s = '';
  for (const b of bytes) s += b.toString(16).padStart(2, '0');
  return s;
}

const HEX_RE = /^(?:[0-9a-f]{2})*$/;

export function fromHex(hex: string): Uint8Array {
  if (!HEX_RE.test(hex)) throw new Error('ожидается hex в нижнем регистре чётной длины');
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export const isHex = (s: unknown, bytes: number): s is string =>
  typeof s === 'string' && s.length === bytes * 2 && HEX_RE.test(s);

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

export function compareBytes(a: Uint8Array, b: Uint8Array): number {
  for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i]! - b[i]!;
  return a.length - b.length;
}

export const sha256 = (data: Uint8Array): Uint8Array => nobleSha256(data);
export const sha256Hex = (data: Uint8Array | string): string =>
  toHex(sha256(typeof data === 'string' ? utf8(data) : data));

/** sha256 канонического JSON — `payloadHash`, `documentHash`, `quoteHash` и т.д. */
export const canonicalHash = (value: unknown): string => sha256Hex(canonicalJson(value));

/** UUID → 16 сырых байт (для листьев дерева и сообщений в программы). */
export function uuidToBytes(uuid: string): Uint8Array {
  const hex = uuid.replace(/-/g, '').toLowerCase();
  if (!isHex(hex, 16)) throw new Error(`некорректный UUID ${uuid}`);
  return fromHex(hex);
}

const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

export function base58Encode(bytes: Uint8Array): string {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let s = '';
  while (n > 0n) {
    s = B58[Number(n % 58n)] + s;
    n /= 58n;
  }
  for (const b of bytes) {
    if (b !== 0) break;
    s = '1' + s;
  }
  return s;
}

export function base58Decode(s: string): Uint8Array {
  let n = 0n;
  for (const c of s) {
    const i = B58.indexOf(c);
    if (i < 0) throw new Error('некорректный base58');
    n = n * 58n + BigInt(i);
  }
  const out: number[] = [];
  while (n > 0n) {
    out.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  for (const c of s) {
    if (c !== '1') break;
    out.unshift(0);
  }
  return Uint8Array.from(out);
}

/** Адрес Solana (base58 32 байта) → байты; бросает при неверной длине. */
export function addressToBytes(address: string): Uint8Array {
  const b = base58Decode(address);
  if (b.length !== 32) throw new Error(`адрес Solana должен быть 32 байта: ${address}`);
  return b;
}
