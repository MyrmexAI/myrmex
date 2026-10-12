import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ed25519 } from '@noble/curves/ed25519.js';
import {
  base58Decode,
  base58Encode,
  buildTree,
  canonicalHash,
  canonicalJson,
  claimLeaf,
  generateDeviceKey,
  getProof,
  open,
  recordLeaf,
  seal,
  toHex,
  verifyProof,
  verifyWalletMessage,
  walletMessageBytes,
  type WorkerAuthorization,
} from '../src/index.ts';
import { ATTESTATION_TAIL_LEN, encodeOracleMessage } from '../src/oracle.ts';

test('canonical JSON: сортировка ключей, без пробелов, отказ от дробей и undefined', () => {
  assert.equal(canonicalJson({ b: 1, a: [true, null, 'x'], c: { z: 0, y: -2 } }), '{"a":[true,null,"x"],"b":1,"c":{"y":-2,"z":0}}');
  assert.throws(() => canonicalJson({ a: 1.5 }));
  assert.throws(() => canonicalJson({ a: undefined }));
  assert.throws(() => canonicalJson({ a: new Date() }));
});

test('конверт: seal → open, подмена payload ловится', () => {
  const key = generateDeviceKey();
  const env = seal(key, 'heartbeat', { v: 1, at: '2026-10-06T00:00:00Z', working: [] });
  const ok = open(env, 'heartbeat');
  assert.equal(ok.ok, true);
  const tampered = { ...env, payload: { ...env.payload, working: ['x'] } };
  assert.deepEqual(open(tampered), { ok: false, error: 'payload_hash_mismatch' });
  assert.deepEqual(open(env, 'hello'), { ok: false, error: 'unexpected_kind' });
  const otherKey = generateDeviceKey();
  assert.deepEqual(open({ ...env, signer: otherKey.publicKey }), { ok: false, error: 'invalid_signature' });
});

test('base58 туда-обратно, включая ведущие нули', () => {
  const bytes = Uint8Array.from([0, 0, 1, 2, 255, ...new Array(27).fill(7)]);
  assert.deepEqual(base58Decode(base58Encode(bytes)), bytes);
  assert.equal(base58Encode(new Uint8Array(32)), '11111111111111111111111111111111');
});

test('подпись кошелька Solana: WorkerAuthorization', () => {
  const sk = ed25519.utils.randomSecretKey();
  const wallet = base58Encode(ed25519.getPublicKey(sk));
  const msg: WorkerAuthorization = {
    v: 1,
    deviceKey: generateDeviceKey().publicKey,
    wallet,
    seatId: base58Encode(new Uint8Array(32).fill(9)),
    nonce: 'ab'.repeat(32),
    expiresAt: 1790000000,
    relayOrigin: 'https://api.example',
  };
  const sig = ed25519.sign(walletMessageBytes('WorkerAuthorization', msg), sk);
  assert.equal(verifyWalletMessage(wallet, 'WorkerAuthorization', msg, base58Encode(sig)), true);
  assert.equal(verifyWalletMessage(wallet, 'WorkerAuthorization', msg, toHex(sig)), true);
  assert.equal(verifyWalletMessage(wallet, 'WorkerAuthorization', { ...msg, expiresAt: 1 }, base58Encode(sig)), false);
});

test('дерево Меркла: доказательство каждого листа, чужой лист не проходит', () => {
  const leaves = Array.from({ length: 7 }, (_, i) =>
    recordLeaf(`0000000${i}-0000-4000-8000-000000000000`, canonicalHash({ i })),
  );
  const tree = buildTree(leaves);
  for (const l of leaves) assert.equal(verifyProof(l, getProof(tree, l), tree.root), true);
  const foreign = recordLeaf('99999999-0000-4000-8000-000000000000', canonicalHash({ i: 1 }));
  assert.equal(verifyProof(foreign, getProof(tree, leaves[0]!), tree.root), false);
  // порядок входа не влияет на корень
  assert.deepEqual(buildTree([...leaves].reverse()).root, tree.root);
});

test('лист выплаты отличается от листа записи (домены 0x02 и 0x00)', () => {
  const wallet = base58Encode(new Uint8Array(32).fill(1));
  const a = claimLeaf(wallet, 5n);
  const b = claimLeaf(wallet, 6n);
  assert.notDeepEqual(a, b);
  const tree = buildTree([a, b]);
  assert.equal(verifyProof(a, getProof(tree, a), tree.root), true);
});

test('ответ оракула: раскладка как в oracle_verifier (домен, потребитель, хвост 102 байта)', () => {
  const answer = new Uint8Array([1]);
  const m = encodeOracleMessage(new Uint8Array(32).fill(5), {
    requestId: new Uint8Array(16).fill(7),
    cluster: 'devnet',
    questionHash: new Uint8Array(32).fill(9),
    answerType: 'bool',
    answer,
    figure: 0n,
    fromSlot: 100n,
    toSlot: 200n,
    slotHash: new Uint8Array(32).fill(3),
    panelJobId: new Uint8Array(16).fill(4),
    panelSize: 20,
    quorum: 14,
    agreed: 15,
    issuedAt: 1_790_000_000n,
    expiresAt: 1_790_086_400n,
  });
  assert.equal(new TextDecoder().decode(m.subarray(0, 15)), 'swarm.oracle.v1');
  assert.equal(m[15], 0);
  assert.equal(m.length, 16 + 32 + 16 + 1 + 32 + 1 + 4 + answer.length + ATTESTATION_TAIL_LEN);
  assert.equal(m[16 + 32 + 16], 1, 'cluster devnet = 1');
  const tail = new DataView(m.buffer, m.length - ATTESTATION_TAIL_LEN);
  assert.equal(tail.getUint16(ATTESTATION_TAIL_LEN - 22, true), 20, 'panelSize (tail::PANEL_SIZE)');
  assert.equal(tail.getUint16(ATTESTATION_TAIL_LEN - 18, true), 15, 'agreed (tail::AGREED)');
  assert.equal(tail.getBigInt64(ATTESTATION_TAIL_LEN - 8, true), 1_790_086_400n, 'expiresAt (tail::EXPIRES_AT)');
});
