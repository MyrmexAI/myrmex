// Подписанные конверты устройства:
// { v, kind, signer, payloadHash, signature, payload }, подпись Ed25519 над
// "<PREFIX>\n<KIND>\n<PAYLOAD_HASH>", payloadHash = sha256(canonicalJson(payload)).

import { ed25519 } from '@noble/curves/ed25519.js';
import { canonicalHash, fromHex, isHex, toHex, utf8 } from './bytes.ts';

/** Префикс домена подписей. Меняется только вместе с версией протокола. */
export const PROTOCOL_PREFIX = 'swarm.v1';

export type Envelope<P = unknown> = {
  v: 1;
  kind: string;
  signer: string;
  payloadHash: string;
  signature: string;
  payload: P;
};

export const signingPreimage = (kind: string, hashHex: string): Uint8Array =>
  utf8(`${PROTOCOL_PREFIX}\n${kind}\n${hashHex}`);

export type DeviceKeypair = { secretKey: Uint8Array; publicKey: string };

export function generateDeviceKey(): DeviceKeypair {
  const secretKey = ed25519.utils.randomSecretKey();
  return { secretKey, publicKey: toHex(ed25519.getPublicKey(secretKey)) };
}

export function deviceKeyFromSecret(secretKey: Uint8Array): DeviceKeypair {
  return { secretKey, publicKey: toHex(ed25519.getPublicKey(secretKey)) };
}

export const signPreimage = (secretKey: Uint8Array, kind: string, hashHex: string): string =>
  toHex(ed25519.sign(signingPreimage(kind, hashHex), secretKey));

export function verifyPreimage(publicKeyHex: string, kind: string, hashHex: string, signatureHex: string): boolean {
  if (!isHex(publicKeyHex, 32) || !isHex(signatureHex, 64) || !isHex(hashHex, 32)) return false;
  try {
    return ed25519.verify(fromHex(signatureHex), signingPreimage(kind, hashHex), fromHex(publicKeyHex));
  } catch {
    return false;
  }
}

export function seal<P>(key: DeviceKeypair, kind: string, payload: P): Envelope<P> {
  const payloadHash = canonicalHash(payload);
  return {
    v: 1,
    kind,
    signer: key.publicKey,
    payloadHash,
    signature: signPreimage(key.secretKey, kind, payloadHash),
    payload,
  };
}

export type OpenResult<P> = { ok: true; envelope: Envelope<P> } | { ok: false; error: string };

/** Проверяет форму, хеш и подпись. Не проверяет, что подписант имеет право — это делает вызывающий. */
export function open<P = unknown>(raw: unknown, expectedKind?: string): OpenResult<P> {
  if (typeof raw !== 'object' || raw === null) return { ok: false, error: 'invalid_envelope' };
  const e = raw as Record<string, unknown>;
  if (e.v !== 1 || typeof e.kind !== 'string' || !('payload' in e)) return { ok: false, error: 'invalid_envelope' };
  if (expectedKind && e.kind !== expectedKind) return { ok: false, error: 'unexpected_kind' };
  if (!isHex(e.signer, 32) || !isHex(e.payloadHash, 32) || !isHex(e.signature, 64)) {
    return { ok: false, error: 'invalid_envelope' };
  }
  let hash: string;
  try {
    hash = canonicalHash(e.payload);
  } catch {
    return { ok: false, error: 'non_canonical_payload' };
  }
  if (hash !== e.payloadHash) return { ok: false, error: 'payload_hash_mismatch' };
  if (!verifyPreimage(e.signer, e.kind, e.payloadHash, e.signature)) return { ok: false, error: 'invalid_signature' };
  return { ok: true, envelope: raw as Envelope<P> };
}
