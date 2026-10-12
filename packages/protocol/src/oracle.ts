// Ответ оракула — ровно тот формат, который проверяет программа `oracle_verifier`:
//   DOMAIN (16) || consumer_program (32) || attestation (Borsh)
// attestation: requestId[16] | cluster u8 | questionHash[32] | answerType u8 | answer: u32 len + bytes |
//              figure u128 | fromSlot u64 | toSlot u64 | slotHash[32] | panelJobId[16] |
//              panelSize u16 | quorum u16 | agreed u16 | issuedAt i64 | expiresAt i64
// Подпись — Ed25519 ключом аттестатора над всем сообщением; потребитель проверяет её через Ed25519 program.

import { utf8 } from './bytes.ts';

export const ORACLE_DOMAIN = (() => {
  const d = new Uint8Array(16);
  d.set(utf8('swarm.oracle.v1'));
  return d;
})();

export const ANSWER_TYPES = ['bool', 'pubkey', 'bytes32', 'u64', 'pubkey[]', 'bytes32[]', 'u128'] as const;
export type AnswerType = (typeof ANSWER_TYPES)[number];

export type OracleAttestation = {
  requestId: Uint8Array; // 16
  cluster: 'mainnet' | 'devnet';
  questionHash: Uint8Array; // 32
  answerType: AnswerType;
  answer: Uint8Array;
  figure: bigint; // u128: число для u64/u128, 0/1 для bool, иначе 0
  fromSlot: bigint;
  toSlot: bigint;
  slotHash: Uint8Array; // 32
  panelJobId: Uint8Array; // 16
  panelSize: number;
  quorum: number;
  agreed: number;
  issuedAt: bigint;
  expiresAt: bigint;
};

/** Длина фиксированного хвоста (как `tail::LEN` в программе). */
export const ATTESTATION_TAIL_LEN = 16 + 8 + 8 + 32 + 16 + 2 + 2 + 2 + 8 + 8;

export function encodeOracleMessage(consumer: Uint8Array, a: OracleAttestation): Uint8Array {
  const fixed = (b: Uint8Array, n: number, what: string) => {
    if (b.length !== n) throw new Error(`${what}: нужно ${n} байт`);
    return b;
  };
  const head = 16 + 32 + 16 + 1 + 32 + 1 + 4 + a.answer.length;
  const out = new Uint8Array(head + ATTESTATION_TAIL_LEN);
  const dv = new DataView(out.buffer);
  let o = 0;
  const put = (b: Uint8Array) => {
    out.set(b, o);
    o += b.length;
  };
  put(ORACLE_DOMAIN);
  put(fixed(consumer, 32, 'consumer'));
  put(fixed(a.requestId, 16, 'requestId'));
  out[o++] = a.cluster === 'mainnet' ? 0 : 1;
  put(fixed(a.questionHash, 32, 'questionHash'));
  out[o++] = ANSWER_TYPES.indexOf(a.answerType);
  dv.setUint32(o, a.answer.length, true);
  o += 4;
  put(a.answer);
  dv.setBigUint64(o, a.figure & 0xffff_ffff_ffff_ffffn, true);
  dv.setBigUint64(o + 8, a.figure >> 64n, true);
  o += 16;
  dv.setBigUint64(o, a.fromSlot, true);
  dv.setBigUint64(o + 8, a.toSlot, true);
  o += 16;
  put(fixed(a.slotHash, 32, 'slotHash'));
  put(fixed(a.panelJobId, 16, 'panelJobId'));
  dv.setUint16(o, a.panelSize, true);
  dv.setUint16(o + 2, a.quorum, true);
  dv.setUint16(o + 4, a.agreed, true);
  o += 6;
  dv.setBigInt64(o, a.issuedAt, true);
  dv.setBigInt64(o + 8, a.expiresAt, true);
  return out;
}
