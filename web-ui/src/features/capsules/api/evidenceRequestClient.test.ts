import { afterEach, describe, expect, it, vi } from 'vitest'
import { askForRecord } from '@/features/capsules/api/evidenceRequestClient'
import { judgeAskReply } from '@/features/capsules/lib/ask-for-record'

afterEach(() => {
  vi.unstubAllGlobals()
})

const PEER = 'c'.repeat(64)
const DIGEST = 'ab'.repeat(32)

/** The tool answers `{answer, request_digest, verification}` (it verifies
 *  by default); `peer-key` answers the announced key. */
function stubRoutes(toolBody: unknown, announced: unknown, askStatus = 200) {
  const fetchMock = vi.fn(async (url: string | URL) =>
    String(url).includes('/http/peer-key')
      ? new Response(JSON.stringify({ announced_key_id: announced }))
      : new Response(JSON.stringify(toolBody), { status: askStatus })
  )
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

describe('askForRecord', () => {
  it("posts a -00 correlation request to this plugin's tool, leaving verification on", async () => {
    const fetchMock = stubRoutes(
      {
        answer: { reason: 'no_such_subject', request_digest: DIGEST },
        request_digest: DIGEST.toUpperCase(),
        verification: { state: 'refusal', reason: 'no_such_subject' }
      },
      'AB'.repeat(32)
    )

    const reply = await askForRecord(PEER, 'nonce-1')

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(String(url)).toBe('/api/plugins/capsule-emit-mesh/tools/mesh_evidence_request')
    expect(init.method).toBe('POST')
    const sent = JSON.parse(String(init.body)) as Record<string, unknown>
    expect(sent).toEqual({
      peer_id: PEER,
      request: { coverage: { min_freshness: 1 }, subject: { correlation: 'nonce-1' } }
    })
    expect('verify' in sent).toBe(false)
    expect(String(fetchMock.mock.calls[1][0])).toBe(`/api/plugins/capsule-emit-mesh/http/peer-key?peer=${PEER}`)
    expect(reply).toEqual({
      kind: 'answer',
      answer: { reason: 'no_such_subject', request_digest: DIGEST },
      verification: { state: 'refusal', reason: 'no_such_subject' },
      requestDigest: DIGEST,
      announcedKeyId: 'ab'.repeat(32)
    })
  })

  it('unwraps an artifact answer and keeps what the verification proved', async () => {
    const answer = { artifact: '{"records":[]}', material: '{}', envelope: {} }
    stubRoutes({ answer, request_digest: DIGEST, verification: { state: 'artifact', anchor: 'x', records: [3, 4], checkpoints: 0 } }, null)
    expect(await askForRecord(PEER, 'nonce-1')).toEqual({
      kind: 'answer',
      answer,
      verification: { state: 'artifact', records: [3, 4] },
      requestDigest: DIGEST,
      announcedKeyId: null
    })
  })

  it('carries an unverified reply as unverified, with the reason', async () => {
    stubRoutes({ answer: {}, request_digest: DIGEST, verification: { state: 'not_evidence', why: 'wrong key' } }, null)
    expect(await askForRecord(PEER, 'nonce-1')).toMatchObject({
      kind: 'answer',
      verification: { state: 'not_evidence', why: 'wrong key' }
    })
    stubRoutes({ answer: {}, request_digest: DIGEST, verification: { state: 'no_announced_key' } }, null)
    expect(await askForRecord(PEER, 'nonce-1')).toMatchObject({ verification: { state: 'no_announced_key' } })
    stubRoutes({ answer: {}, request_digest: DIGEST, verification: { state: 'something new' } }, null)
    expect(await askForRecord(PEER, 'nonce-1')).toMatchObject({ verification: { state: 'unknown' } })
  })

  it('an old, unwrapped reply is no answer, never read as one', async () => {
    stubRoutes({ reason: 'no_such_subject' }, null)
    expect(await askForRecord(PEER, 'nonce-1')).toEqual({ kind: 'no_answer', message: 'the reply carried no answer' })
  })

  it('a tool error is no answer, never an answer', async () => {
    stubRoutes('could not reach peer', null, 502)
    expect(await askForRecord(PEER, 'nonce-1')).toMatchObject({ kind: 'no_answer' })
  })

  it('end to end: a verified artifact from the tool becomes their record on the row', async () => {
    const theirs = { capsule_id: 'theirs-1', effect: { request_digest: 'e'.repeat(64) } }
    const body = Array.from(new TextEncoder().encode(JSON.stringify(theirs)), (b) => b.toString(16).padStart(2, '0')).join('')
    const answer = {
      artifact: JSON.stringify({ records: [{ leaf_index: 7, digest: 'd'.repeat(64), body }] }),
      material: '{}',
      envelope: {}
    }
    stubRoutes({ answer, request_digest: DIGEST, verification: { state: 'artifact', records: [7] } }, 'ab'.repeat(32))
    const outcome = await judgeAskReply(await askForRecord(PEER, 'nonce-1'), 'e'.repeat(64), '2026-09-28T23:00:00Z', {
      recomputeIdMatch: async () => true,
      producerSignatureVerifies: () => true
    })
    expect(outcome).toMatchObject({ kind: 'record', evidence: { status: 'found', peerRecord: theirs } })
  })
})
