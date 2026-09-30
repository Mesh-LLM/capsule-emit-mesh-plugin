// Ask the other side of an exchange for its record: this plugin's
// `mesh_evidence_request` tool (`tools/mesh_evidence_request`) sends one
// evidence request (draft-mih-agent-evidence-request-00) to that node over
// the mesh. The tool verifies what comes back before handing it over
// (`evidence_answer::verify_response`, under the key the operator announced
// for that node in `ADMISSION_POLICY_PEER_KEYS`) and answers
// `{answer, request_digest, verification}`: the peer's reply unchanged, the
// digest of the request bytes it actually sent, and what the reply proved.
// This client carries that; `ask-for-record.ts` judges it.
//
// Ported from the fork console, which reached the same tool through a host
// route (`POST /api/evidence-requests`) that also looked up the key.
import { getPluginJson, pluginHost } from '@/plugin-host/host'

export const EVIDENCE_REQUEST_ROUTE = 'tools/mesh_evidence_request'
export const peerKeyRoute = (peerId: string) => `http/peer-key?peer=${encodeURIComponent(peerId)}`

/** What the tool's verification found (`evidence_answer::verify_response`):
 *  a refusal signed with the announced key for our request; an artifact that
 *  verifies (the served records, by leaf index); a reply that proves nothing;
 *  or no announced key to check it under. `unknown` for anything else. */
export type EvidenceVerification =
  | { state: 'refusal'; reason: string }
  | { state: 'artifact'; records: number[] }
  | { state: 'not_evidence'; why: string }
  | { state: 'no_announced_key' }
  | { state: 'unknown' }

/** An answer comes with what it must be judged against: the tool's
 *  verification; the digest of the request bytes the tool sent, which a
 *  refusal must name; and the key this node was told the peer signs with
 *  (`null` when none is announced), which the record's own signature is
 *  checked under. */
export type EvidenceAskReply =
  | {
      kind: 'answer'
      answer: unknown
      verification: EvidenceVerification
      requestDigest: string
      announcedKeyId: string | null
    }
  | { kind: 'no_answer'; message: string }

/** The -00 request for the records carrying one client nonce: a correlation
 *  subject, under the freshest checkpoint the peer holds. */
export function askByNonceRequest(nonce: string): Record<string, unknown> {
  return { coverage: { min_freshness: 1 }, subject: { correlation: nonce } }
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null
}

function parseVerification(value: unknown): EvidenceVerification {
  const v = record(value)
  switch (v?.state) {
    case 'refusal':
      return typeof v.reason === 'string' ? { state: 'refusal', reason: v.reason } : { state: 'unknown' }
    case 'artifact':
      return Array.isArray(v.records) && v.records.every((i) => Number.isInteger(i))
        ? { state: 'artifact', records: v.records as number[] }
        : { state: 'unknown' }
    case 'not_evidence':
      return { state: 'not_evidence', why: typeof v.why === 'string' ? v.why : 'it did not verify' }
    case 'no_announced_key':
      return { state: 'no_announced_key' }
    default:
      return { state: 'unknown' }
  }
}

/** The key this node was told `peerId` signs with, or `null` when none is
 *  announced or the route can't say: an unknown key is never a guess. */
async function announcedKeyFor(peerId: string): Promise<string | null> {
  try {
    const body = await getPluginJson<{ announced_key_id?: unknown }>(peerKeyRoute(peerId))
    return typeof body.announced_key_id === 'string' ? body.announced_key_id.toLowerCase() : null
  } catch {
    return null
  }
}

export async function askForRecord(peerId: string, nonce: string): Promise<EvidenceAskReply> {
  const request = askByNonceRequest(nonce)
  let response: Response
  try {
    // No `verify` member: the tool verifies unless told not to.
    response = await pluginHost().network.fetchPlugin(EVIDENCE_REQUEST_ROUTE, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ peer_id: peerId, request })
    })
  } catch (error) {
    return { kind: 'no_answer', message: error instanceof Error ? error.message : 'network error' }
  }
  if (!response.ok) {
    const text = await response.text().catch(() => '')
    return { kind: 'no_answer', message: text || `HTTP ${response.status}` }
  }
  let body: unknown
  try {
    body = await response.json()
  } catch {
    return { kind: 'no_answer', message: 'the reply was not JSON' }
  }
  const reply = record(body)
  const answer = record(reply?.answer)
  const requestDigest = typeof reply?.request_digest === 'string' ? reply.request_digest.toLowerCase() : null
  if (!answer || !requestDigest || !/^[0-9a-f]{64}$/.test(requestDigest))
    return { kind: 'no_answer', message: 'the reply carried no answer' }
  return {
    kind: 'answer',
    answer,
    verification: parseVerification(reply?.verification),
    requestDigest,
    announcedKeyId: await announcedKeyFor(peerId)
  }
}
