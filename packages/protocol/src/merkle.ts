// Деревья Меркла. Правила зафиксированы и не меняются:
//   лист записи    = sha256(0x00 || jobId(16) || documentHash(32))
//   лист выплаты   = sha256(0x02 || wallet(32) || cumulativeAmount(u64 LE))
//   внутренний узел = sha256(0x01 || min(a,b) || max(a,b))   — сортированные пары
// Та же логика проверяется в программах records_root и distributor.

import { addressToBytes, compareBytes, concat, fromHex, sha256, toHex, uuidToBytes } from './bytes.ts';

const LEAF_RECORD = 0x00;
const NODE = 0x01;
const LEAF_CLAIM = 0x02;

export function recordLeaf(jobId: string, documentHashHex: string): Uint8Array {
  return sha256(concat(Uint8Array.of(LEAF_RECORD), uuidToBytes(jobId), fromHex(documentHashHex)));
}

export function u64le(n: bigint): Uint8Array {
  if (n < 0n || n > 0xffff_ffff_ffff_ffffn) throw new Error('u64 вне диапазона');
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, n, true);
  return out;
}

export function claimLeaf(wallet: string, cumulativeAmount: bigint): Uint8Array {
  return sha256(concat(Uint8Array.of(LEAF_CLAIM), addressToBytes(wallet), u64le(cumulativeAmount)));
}

export function hashPair(a: Uint8Array, b: Uint8Array): Uint8Array {
  const [lo, hi] = compareBytes(a, b) <= 0 ? [a, b] : [b, a];
  return sha256(concat(Uint8Array.of(NODE), lo, hi));
}

export type MerkleTree = { root: Uint8Array; layers: Uint8Array[][] };

/** Листья сортируются, поэтому корень не зависит от порядка входа. Нечётный узел поднимается без пары. */
export function buildTree(leaves: Uint8Array[]): MerkleTree {
  if (leaves.length === 0) throw new Error('пустое дерево');
  const sorted = [...leaves].sort(compareBytes);
  const layers: Uint8Array[][] = [sorted];
  while (layers.at(-1)!.length > 1) {
    const prev = layers.at(-1)!;
    const next: Uint8Array[] = [];
    for (let i = 0; i < prev.length; i += 2) next.push(i + 1 < prev.length ? hashPair(prev[i]!, prev[i + 1]!) : prev[i]!);
    layers.push(next);
  }
  return { root: layers.at(-1)![0]!, layers };
}

export function getProof(tree: MerkleTree, leaf: Uint8Array): Uint8Array[] {
  let idx = tree.layers[0]!.findIndex((l) => compareBytes(l, leaf) === 0);
  if (idx < 0) throw new Error('лист не найден');
  const proof: Uint8Array[] = [];
  for (let d = 0; d < tree.layers.length - 1; d++) {
    const layer = tree.layers[d]!;
    const sib = idx ^ 1;
    if (sib < layer.length) proof.push(layer[sib]!);
    idx >>= 1;
  }
  return proof;
}

export function verifyProof(leaf: Uint8Array, proof: Uint8Array[], root: Uint8Array): boolean {
  let h = leaf;
  for (const p of proof) h = hashPair(h, p);
  return compareBytes(h, root) === 0;
}

export const proofToHex = (proof: Uint8Array[]): string[] => proof.map(toHex);
