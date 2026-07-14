import { test } from 'node:test'
import assert from 'node:assert/strict'
import { base64ToBytes, makeBinaryResponse } from './mockApi-binary.ts'

// TEST-6 (covers ITEM-11): the binary-response path returns real bytes with
// the right content-type, not JSON.

test('makeBinaryResponse returns application/pdf bytes', async () => {
  const bytes = new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d]) // "%PDF-"
  const res = makeBinaryResponse(bytes, 'application/pdf')
  assert.equal(res.status, 200)
  assert.equal(res.headers.get('content-type'), 'application/pdf')
  assert.equal(res.headers.get('content-length'), '5')
  const ab = await res.arrayBuffer()
  assert.deepEqual(new Uint8Array(ab), bytes)
})

test('base64ToBytes round-trips through makeBinaryResponse', async () => {
  // base64 of "%PDF-1.4"
  const b64 = 'JVBERi0xLjQ='
  const bytes = base64ToBytes(b64)
  const res = makeBinaryResponse(bytes, 'application/pdf')
  const ab = await res.arrayBuffer()
  assert.equal(new TextDecoder().decode(ab), '%PDF-1.4')
})
