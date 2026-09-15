<script setup lang="ts">
import { computed } from 'vue'
import type { KeyModelBreakdown } from '../composables/useUsageHistory'
import { formatCost } from '../utils/format'

const props = defineProps<{
  entries: KeyModelBreakdown[]
  formatTokens: (v: number) => string
}>()

type Row = KeyModelBreakdown & { totalTokens: number }

type Group = {
  keyId: string
  label: string
  rows: Row[]
  requestCount: number
  costMicrodollars: number
  totalTokens: number
}

/**
 * Pivot the flat (key, model) rows into one group per key, each sorted by cost.
 * Groups themselves are ordered by total cost so the heaviest users come first.
 */
const groups = computed<Group[]>(() => {
  const byKey = new Map<string, Group>()

  for (const entry of props.entries) {
    const totalTokens =
      entry.inputTokens + entry.outputTokens + entry.cacheReadTokens + entry.cacheWriteTokens

    let group = byKey.get(entry.keyId)
    if (!group) {
      group = {
        keyId: entry.keyId,
        label: entry.keyName ?? entry.keyId.slice(0, 8),
        rows: [],
        requestCount: 0,
        costMicrodollars: 0,
        totalTokens: 0,
      }
      byKey.set(entry.keyId, group)
    }

    group.rows.push({ ...entry, totalTokens })
    group.requestCount += entry.requestCount
    group.costMicrodollars += entry.costMicrodollars
    group.totalTokens += totalTokens
  }

  const groupList = [...byKey.values()]
  for (const group of groupList) {
    group.rows.sort((a, b) => b.costMicrodollars - a.costMicrodollars)
  }
  groupList.sort((a, b) => b.costMicrodollars - a.costMicrodollars)
  return groupList
})

/** Share of the key's cost that went to a single model, for the inline bar. */
function costShare(row: Row, group: Group): number {
  if (group.costMicrodollars === 0) return 0
  return (row.costMicrodollars / group.costMicrodollars) * 100
}
</script>

<template>
  <div class="overflow-x-auto">
    <table class="w-full text-sm">
      <thead>
        <tr class="text-gray-500 dark:text-gray-400 text-xs uppercase tracking-wide">
          <th class="text-left font-medium py-2 pr-3">Key / model</th>
          <th class="text-right font-medium py-2 px-3">Requests</th>
          <th class="text-right font-medium py-2 px-3">Input</th>
          <th class="text-right font-medium py-2 px-3">Output</th>
          <th class="text-right font-medium py-2 px-3">Cache R</th>
          <th class="text-right font-medium py-2 px-3">Cache W</th>
          <th class="text-right font-medium py-2 pl-3">Cost</th>
        </tr>
      </thead>
      <tbody>
        <template v-for="group in groups" :key="group.keyId">
          <tr class="border-t border-gray-200 dark:border-gray-800 bg-gray-50 dark:bg-gray-900/50">
            <td class="py-2 pr-3 font-medium">
              {{ group.label }}
              <span class="text-gray-400 dark:text-gray-500 font-normal">
                · {{ group.rows.length }}
                {{ group.rows.length === 1 ? 'model' : 'models' }}
              </span>
            </td>
            <td class="py-2 px-3 text-right tabular-nums">
              {{ group.requestCount.toLocaleString() }}
            </td>
            <td class="py-2 px-3 text-right text-gray-400 dark:text-gray-500" colspan="4">
              {{ formatTokens(group.totalTokens) }} tokens
            </td>
            <td class="py-2 pl-3 text-right font-medium tabular-nums">
              {{ formatCost(group.costMicrodollars) }}
            </td>
          </tr>

          <tr
            v-for="row in group.rows"
            :key="`${group.keyId}:${row.model}`"
            class="border-t border-gray-100 dark:border-gray-800/50"
          >
            <td class="py-1.5 pr-3 pl-4">
              <div class="flex items-center gap-2">
                <span class="truncate">{{ row.model }}</span>
                <span class="text-xs text-gray-400 dark:text-gray-500 tabular-nums">
                  {{ costShare(row, group).toFixed(0) }}%
                </span>
              </div>
              <div class="mt-1 h-1 rounded bg-gray-100 dark:bg-gray-800 overflow-hidden">
                <div
                  class="h-full rounded bg-primary-500"
                  :style="{ width: `${costShare(row, group)}%` }"
                />
              </div>
            </td>
            <td class="py-1.5 px-3 text-right tabular-nums">
              {{ row.requestCount.toLocaleString() }}
            </td>
            <td class="py-1.5 px-3 text-right tabular-nums">
              {{ formatTokens(row.inputTokens) }}
            </td>
            <td class="py-1.5 px-3 text-right tabular-nums">
              {{ formatTokens(row.outputTokens) }}
            </td>
            <td class="py-1.5 px-3 text-right tabular-nums">
              {{ formatTokens(row.cacheReadTokens) }}
            </td>
            <td class="py-1.5 px-3 text-right tabular-nums">
              {{ formatTokens(row.cacheWriteTokens) }}
            </td>
            <td class="py-1.5 pl-3 text-right tabular-nums">
              {{ formatCost(row.costMicrodollars) }}
            </td>
          </tr>
        </template>
      </tbody>
    </table>
  </div>
</template>
