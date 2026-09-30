// Payments on an exchange row. The face shows the `paid` chip, the payer-book
// state and the one fact about the other book; the checks expansion lists each
// recorded step with who stated it and the amount as recorded. Chips are
// HoverChips (spans), never buttons: the row's button count is pinned.
import { StatusBadge } from '@/components/ui/StatusBadge'
import type { PayerBook } from '@/features/capsules/api/sidecarTypes'
import { HoverChip } from '@/features/capsules/components/HoverChip'
import { settlementEntryViews, settlementRowView } from '@/features/capsules/lib/settlement-view'
import { shortId } from '@/features/capsules/lib/short-id'
import {
  SETTLEMENT_PAID_TOOLTIP,
  SETTLEMENT_TERMS_TOOLTIP,
  SETTLEMENT_PROVIDER_BOOK_TOOLTIP
} from '@/features/capsules/lib/tooltip-copy'

export function SettlementStrip({ settlement }: { settlement: PayerBook | null | undefined }) {
  const view = settlementRowView(settlement)
  if (!view) return null
  return (
    <div
      aria-label="payment"
      className="flex flex-wrap items-center gap-2 text-xs"
      data-settlement-state={view.stateKey}
      role="group"
    >
      <HoverChip
        census={`settlement:${view.chip}`}
        label={view.chip === 'paid' ? SETTLEMENT_PAID_TOOLTIP : SETTLEMENT_TERMS_TOOLTIP}
      >
        <span>
          <StatusBadge size="caption" tone={view.chip === 'paid' ? 'accent' : 'muted'}>
            {view.chipLabel}
          </StatusBadge>
        </span>
      </HoverChip>
      <HoverChip census={`settlement_state:${view.stateKey}`} label={view.tooltip}>
        <span>
          <StatusBadge size="caption" tone={view.tone}>
            {view.label}
          </StatusBadge>
        </span>
      </HoverChip>
      <HoverChip census="settlement:provider_book" label={SETTLEMENT_PROVIDER_BOOK_TOOLTIP}>
        <span className="text-fg-faint">{view.providerBook}</span>
      </HoverChip>
      {view.termsNote ? <span className="text-fg-faint">{view.termsNote}</span> : null}
    </div>
  )
}

export function SettlementEntries({ settlement }: { settlement: PayerBook | null | undefined }) {
  if (!settlement || settlement.entries.length === 0) return null
  const entries = settlementEntryViews(settlement.entries)
  return (
    <div
      aria-label="payment records"
      className="flex flex-col gap-1 rounded border border-border-soft px-3 py-2"
      role="group"
    >
      <p className="type-caption text-fg-faint">Payment records, as this node sealed them</p>
      <ul className="flex flex-col gap-1">
        {entries.map((entry) => (
          <li
            className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs"
            data-settlement-entry={entry.phase}
            key={entry.key}
          >
            <span className="text-foreground">{entry.phase}</span>
            <HoverChip census={`settlement_source:${entry.sourceKey ?? 'unrecognised'}`} label={entry.sourceTooltip}>
              <span className="rounded border border-border-soft px-1 text-[11px] text-fg-dim">
                {entry.sourceLabel}
              </span>
            </HoverChip>
            {entry.amount ? <span className="font-mono tabular-nums text-fg-dim">{entry.amount}</span> : null}
            {entry.paymentHash ? (
              <span className="font-mono text-fg-faint" title={entry.paymentHash}>
                payment {shortId(entry.paymentHash)}
              </span>
            ) : null}
            {entry.referenceNote ? <span className="text-fg-faint">{entry.referenceNote}</span> : null}
          </li>
        ))}
      </ul>
    </div>
  )
}
