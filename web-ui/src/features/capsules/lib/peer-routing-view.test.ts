import { describe, expect, it } from 'vitest'
import type { PaneBRow } from '@/features/capsules/api/sidecarTypes'
import { HARNESS_PANE_B_PAYLOAD } from '@/features/capsules/lib/peer-fixtures'
import { dealingsLines } from '@/features/capsules/lib/peer-routing-view'

describe('dealingsLines -- the drill says what the Peers row says', () => {
  it('your dealings: counts with their denominators, disputes said as none when none were judged', () => {
    const [clean, alarmed] = HARNESS_PANE_B_PAYLOAD.rows
    expect(dealingsLines(clean)).toEqual([
      '24 exchanges with you · they confirmed 16 of 24',
      'Same request & answer: 16 · 0 differ',
      'Disputes judged: 8 of 24 · 8 corroborated'
    ])
    expect(dealingsLines(alarmed)[1]).toMatch(/· 1 differ$/)
    const unjudged: PaneBRow = { ...clean, verdicts: { state: 'NOT_CHECKED' } }
    expect(dealingsLines(unjudged)[2]).toBe('Disputes judged: none')
  })

  it('a half the door refused for contradicting your record is a disagreement, never "0 differ"', () => {
    const row: PaneBRow = {
      ...HARNESS_PANE_B_PAYLOAD.rows[0],
      exchange_count: 3,
      confirmed_siblings: [
        {
          mine: { state: 'present-unverified', capsule_id: 'a'.repeat(64) },
          theirs: {
            state: 'present-unverified',
            capsule_id: null,
            evidence_outcome: 'claims_refused',
            evidence_outcome_date: '2026-09-28T08:00:00Z'
          }
        }
      ]
    }
    expect(dealingsLines(row).slice(0, 2)).toEqual([
      '3 exchanges with you · they confirmed 0 of 3',
      'Same request & answer: 0 · 1 differ'
    ])
  })
})
