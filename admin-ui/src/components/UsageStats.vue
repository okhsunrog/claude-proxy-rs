<script setup lang="ts">
import { computed } from 'vue'
import type { KeyUsageResponse } from '../composables/useKeys'

const props = defineProps<{
  usage: KeyUsageResponse | undefined
}>()

function formatCost(microdollars: number): string {
  const dollars = microdollars / 1_000_000
  if (dollars >= 1000) return `$${(dollars / 1000).toFixed(1)}K`
  if (dollars >= 100) return `$${dollars.toFixed(0)}`
  if (dollars >= 1) return `$${dollars.toFixed(2)}`
  if (dollars >= 0.01) return `$${dollars.toFixed(3)}`
  if (microdollars === 0) return '$0'
  return `$${dollars.toFixed(4)}`
}

function formatDateTime(timestamp: number): string {
  return new Date(timestamp).toLocaleString()
}

function barColor(percentage: number): string {
  if (percentage >= 90) return 'error'
  if (percentage >= 70) return 'warning'
  return 'success'
}

interface UsageItem {
  label: string
  used: number
  limit: number | null | undefined
  resetAt?: number
}

const items = computed<UsageItem[]>(() => {
  if (!props.usage) return []
  const { usage, limits } = props.usage
  return [
    {
      label: 'Hourly',
      used: usage.hourlyTokens ?? 0,
      limit: limits.hourlyLimit,
      resetAt: usage.hourlyResetAt,
    },
    {
      label: 'Weekly',
      used: usage.weeklyTokens ?? 0,
      limit: limits.weeklyLimit,
      resetAt: usage.weeklyResetAt,
    },
    {
      label: 'Total',
      used: usage.totalTokens ?? 0,
      limit: limits.totalLimit,
    },
  ]
})
</script>

<template>
  <div v-if="!usage" class="text-sm text-muted">Loading usage...</div>
  <div v-else class="grid grid-cols-3 gap-3">
    <div
      v-for="item in items"
      :key="item.label"
      class="rounded-lg bg-elevated p-3"
    >
      <div class="text-xs text-muted uppercase">{{ item.label }}</div>
      <div class="text-xl font-semibold mt-1">{{ formatCost(item.used) }}</div>
      <div class="text-xs text-muted">
        / {{ item.limit ? formatCost(item.limit) : 'Unlimited' }}
      </div>
      <UProgress
        v-if="item.limit"
        :model-value="Math.min((item.used / item.limit) * 100, 100)"
        :color="barColor((item.used / item.limit) * 100)"
        size="xs"
        class="mt-2"
      />
      <div v-if="item.resetAt" class="text-xs text-muted mt-1">
        Resets: {{ formatDateTime(item.resetAt) }}
      </div>
    </div>
  </div>
</template>
