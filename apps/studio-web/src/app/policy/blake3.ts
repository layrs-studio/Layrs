/*
 * BLAKE3 (hash mode, 32-byte output) for the object identities the server
 * recomputes: evidence, Policy products, PolicySets and request digests. The
 * server verifies every identity, so a mismatch is a refusal, never a grant.
 */

const IV = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
const PERMUTATION = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1;
const CHUNK_END = 2;
const PARENT = 4;
const ROOT = 8;
const BLOCK_LEN = 64;
const CHUNK_LEN = 1024;

function g(state: Uint32Array, a: number, b: number, c: number, d: number, x: number, y: number) {
  state[a] = (state[a] + state[b] + x) >>> 0;
  state[d] = rotr(state[d] ^ state[a], 16);
  state[c] = (state[c] + state[d]) >>> 0;
  state[b] = rotr(state[b] ^ state[c], 12);
  state[a] = (state[a] + state[b] + y) >>> 0;
  state[d] = rotr(state[d] ^ state[a], 8);
  state[c] = (state[c] + state[d]) >>> 0;
  state[b] = rotr(state[b] ^ state[c], 7);
}

function rotr(value: number, bits: number) {
  return ((value >>> bits) | (value << (32 - bits))) >>> 0;
}

/** Returns the 16-word output of one compression. */
function compress(cv: Uint32Array, block: Uint32Array, counter: number, blockLen: number, flags: number) {
  const state = new Uint32Array([
    cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7],
    IV[0], IV[1], IV[2], IV[3],
    counter >>> 0, Math.floor(counter / 0x100000000) >>> 0, blockLen, flags,
  ]);
  let m = block;
  for (let round = 0; round < 7; round++) {
    g(state, 0, 4, 8, 12, m[0], m[1]);
    g(state, 1, 5, 9, 13, m[2], m[3]);
    g(state, 2, 6, 10, 14, m[4], m[5]);
    g(state, 3, 7, 11, 15, m[6], m[7]);
    g(state, 0, 5, 10, 15, m[8], m[9]);
    g(state, 1, 6, 11, 12, m[10], m[11]);
    g(state, 2, 7, 8, 13, m[12], m[13]);
    g(state, 3, 4, 9, 14, m[14], m[15]);
    if (round < 6) {
      const next = new Uint32Array(16);
      for (let index = 0; index < 16; index++) next[index] = m[PERMUTATION[index]];
      m = next;
    }
  }
  for (let index = 0; index < 8; index++) {
    state[index] ^= state[index + 8];
    state[index + 8] ^= cv[index];
  }
  return state;
}

function words(bytes: Uint8Array, offset: number, length: number) {
  const block = new Uint8Array(BLOCK_LEN);
  block.set(bytes.subarray(offset, offset + length));
  const view = new DataView(block.buffer);
  const out = new Uint32Array(16);
  for (let index = 0; index < 16; index++) out[index] = view.getUint32(index * 4, true);
  return out;
}

type Output = { cv: Uint32Array; block: Uint32Array; counter: number; blockLen: number; flags: number };

/** The last block of a chunk, not yet compressed so the root flag can be added. */
function chunkOutput(bytes: Uint8Array, start: number, end: number, counter: number): Output {
  let cv: Uint32Array = IV.slice();
  const length = end - start;
  const blocks = Math.max(1, Math.ceil(length / BLOCK_LEN));
  for (let index = 0; index < blocks - 1; index++) {
    const flags = index === 0 ? CHUNK_START : 0;
    cv = compress(cv, words(bytes, start + index * BLOCK_LEN, BLOCK_LEN), counter, BLOCK_LEN, flags).slice(0, 8);
  }
  const lastOffset = start + (blocks - 1) * BLOCK_LEN;
  const lastLen = end - lastOffset;
  return {
    cv,
    block: words(bytes, lastOffset, lastLen),
    counter,
    blockLen: lastLen,
    flags: (blocks === 1 ? CHUNK_START : 0) | CHUNK_END,
  };
}

function chainingValue(output: Output) {
  return compress(output.cv, output.block, output.counter, output.blockLen, output.flags).slice(0, 8);
}

function parentOutput(left: Uint32Array, right: Uint32Array): Output {
  const block = new Uint32Array(16);
  block.set(left, 0);
  block.set(right, 8);
  return { cv: IV.slice(), block, counter: 0, blockLen: BLOCK_LEN, flags: PARENT };
}

export function blake3(input: Uint8Array): Uint8Array {
  const chunks = Math.max(1, Math.ceil(input.length / CHUNK_LEN));
  const stack: Uint32Array[] = [];
  let output: Output | null = null;
  for (let chunk = 0; chunk < chunks; chunk++) {
    const start = chunk * CHUNK_LEN;
    const current = chunkOutput(input, start, Math.min(input.length, start + CHUNK_LEN), chunk);
    if (chunk === chunks - 1) {
      // Fold the final chunk into the pending subtrees, right to left.
      output = current;
      while (stack.length) output = parentOutput(stack.pop()!, chainingValue(output));
    } else {
      let cv = chainingValue(current);
      // Merge completed subtrees: one merge per trailing zero of the chunk count.
      let total = chunk + 1;
      while ((total & 1) === 0) {
        cv = chainingValue(parentOutput(stack.pop()!, cv));
        total >>= 1;
      }
      stack.push(cv);
    }
  }
  const root = compress(output!.cv, output!.block, output!.counter, output!.blockLen, output!.flags | ROOT);
  const digest = new Uint8Array(32);
  const view = new DataView(digest.buffer);
  for (let index = 0; index < 8; index++) view.setUint32(index * 4, root[index], true);
  return digest;
}

export function hex(bytes: Uint8Array) {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

const encoder = new TextEncoder();

/** `blake3:<hex>` of a domain label, a NUL separator (when given) and the bytes. */
export function objectId(domain: string, bytes: Uint8Array, separator = true) {
  const prefix = encoder.encode(domain);
  const input = new Uint8Array(prefix.length + (separator ? 1 : 0) + bytes.length);
  input.set(prefix, 0);
  input.set(bytes, prefix.length + (separator ? 1 : 0));
  return `blake3:${hex(blake3(input))}`;
}
