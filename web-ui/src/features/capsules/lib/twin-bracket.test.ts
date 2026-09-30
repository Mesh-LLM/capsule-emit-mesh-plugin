import { describe, expect, it } from 'vitest'
import {
  twinComparisonParametersLine,
  twinDisclosureSentence,
  twinResponseTexts
} from '@/features/capsules/lib/twin-bracket'
import type { ExchangeLedgerRow } from '@/features/capsules/lib/exchange-ledger'
import type { PaneCRow } from '@/features/capsules/api/sidecarTypes'

function twinRow(overrides: Partial<PaneCRow> = {}): ExchangeLedgerRow {
  const raw: PaneCRow = {
    exchange_key: 'exch-1',
    role_tag: 'ASKED',
    header_state: 'ok',
    properties: null,
    has_issue: false,
    mine: { state: 'present', capsule_id: null },
    theirs: { state: 'present', capsule_id: null },
    unilateral: false,
    timestamp: '2026-09-14T00:00:00Z',
    twin_bracket_id: 'twin-xyz',
    ...overrides
  }
  return {
    exchangeKey: raw.exchange_key,
    timestamp: raw.timestamp,
    roleTag: raw.role_tag,
    counterparty: null,
    confirmed: false,
    hasIssue: false,
    checksText: '—',
    rightCellState: { kind: 'closed', date: null },
    contentToggleState: { your: { kind: 'populated', date: null }, their: { kind: 'not_asked', date: null } },
    sessionId: null,
    twinBracketId: raw.twin_bracket_id ?? null,
    raw
  }
}

describe('twinDisclosureSentence — v3 §5 "Ambient twins are unannounced, and the ledger says so"', () => {
  it('uses the LIVE rate passed in, not a hardcoded 50 -- two different N values produce two different sentences', () => {
    expect(twinDisclosureSentence(50)).toBe(
      'This comparison ran automatically — 1 in 50 exchanges is sent to a second peer.'
    )
    expect(twinDisclosureSentence(2)).toBe(
      'This comparison ran automatically — 1 in 2 exchanges is sent to a second peer.'
    )
  })

  it('degrades honestly (never fabricates an N) when the live rate is unavailable', () => {
    expect(twinDisclosureSentence(null)).toBe('This comparison ran automatically — sent to a second peer.')
  })
})

describe('twinComparisonParametersLine', () => {
  it('joins every field the payload actually carries', () => {
    const rows = [
      twinRow({
        exchange_key: 'a',
        twin_comparison: { temperature: 0, seed: 1, model_identity_hash: 'd41d…', settings_label: 'KV F16/F16' }
      }),
      twinRow({ exchange_key: 'b' })
    ]
    expect(twinComparisonParametersLine(rows)).toBe('temp 0 · seed 1 · model identity d41d… · settings KV F16/F16')
  })

  it('skips missing fields rather than fabricating placeholders', () => {
    const rows = [twinRow({ exchange_key: 'a', twin_comparison: { temperature: 0.7 } }), twinRow({ exchange_key: 'b' })]
    expect(twinComparisonParametersLine(rows)).toBe('temp 0.7')
  })

  it('returns null when neither row carries comparison data -- never an empty line', () => {
    const rows = [twinRow({ exchange_key: 'a' }), twinRow({ exchange_key: 'b' })]
    expect(twinComparisonParametersLine(rows)).toBeNull()
  })
})

describe('twinResponseTexts', () => {
  it("returns each side's real response text for the Compare diff", () => {
    const rows = [
      twinRow({ exchange_key: 'a', mine: { state: 'present', capsule_id: null, text: 'hello from peer A' } }),
      twinRow({ exchange_key: 'b', mine: { state: 'present', capsule_id: null, text: 'hello from peer B' } })
    ]
    expect(twinResponseTexts(rows)).toEqual(['hello from peer A', 'hello from peer B'])
  })

  it('is null on a side that never populated text -- never invented content to diff against', () => {
    const rows = [twinRow({ exchange_key: 'a' }), twinRow({ exchange_key: 'b' })]
    expect(twinResponseTexts(rows)).toEqual([null, null])
  })
})
