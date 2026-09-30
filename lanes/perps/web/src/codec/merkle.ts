/** Merkle proof verification for display (spec §9.9): SHA-256, 0x01-prefixed nodes, zero-padded to a power of two. */
import { concat, sha256 } from "./bytes";

const ZERO = new Uint8Array(32);

function node(l: Uint8Array, r: Uint8Array): Uint8Array {
  return sha256(concat(Uint8Array.of(0x01), l, r));
}

export function depth(n: number): number {
  let d = 0;
  while (1 << d < n) d++;
  return d;
}

export function verify(leaf: Uint8Array, index: number, count: number, proof: Uint8Array[], root: Uint8Array): boolean {
  if (index >= count || proof.length !== depth(count)) return false;
  let h = leaf;
  let i = index;
  for (const s of proof) {
    h = i % 2 === 0 ? node(h, s) : node(s, h);
    i = Math.floor(i / 2);
  }
  return h.length === root.length && h.every((b, j) => b === root[j]);
}

export { ZERO };
