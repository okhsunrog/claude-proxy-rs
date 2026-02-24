<script setup lang="ts">
import { onMounted, computed } from 'vue'
import { AreaChart } from 'vue-chrts'
import CategoryDistribution from './CategoryDistribution.vue'
import { useUsageHistory, type Period } from '../composables/useUsageHistory'
import { formatCost } from '../utils/format'

const {
  period,
  timeseries,
  byModel,
  byKey,
  isLoading,
  totalCost,
  totalRequests,
  avgCostPerRequest,
  totalInputTokens,
  totalOutputTokens,
  totalCacheReadTokens,
  totalCacheWriteTokens,
  totalTokens,
  fetchAll,
  clearHistory,
  setPeriod,
} = useUsageHistory()

const periods: { label: string; value: Period }[] = [
  { label: '24h', value: '24h' },
  { label: '7d', value: '7d' },
  { label: '30d', value: '30d' },
]

// Area chart data: cost over time
const areaData = computed(() =>
  timeseries.value.map((p) => ({
    x: p.timestamp,
    cost: p.costMicrodollars / 1_000_000,
  })),
)

const areaCategories = computed(() => ({
  cost: { name: 'Cost ($)', color: '#22c55e' },
}))

// Area chart data: tokens over time
const tokenAreaData = computed(() =>
  timeseries.value.map((p) => ({
    x: p.timestamp,
    input: p.inputTokens,
    output: p.outputTokens,
    cacheRead: p.cacheReadTokens,
    cacheWrite: p.cacheWriteTokens,
  })),
)

const tokenAreaCategories = computed(() => ({
  input: { name: 'Input', color: '#3b82f6' },
  output: { name: 'Output', color: '#22c55e' },
  cacheRead: { name: 'Cache Read', color: '#f59e0b' },
  cacheWrite: { name: 'Cache Write', color: '#8b5cf6' },
}))

function formatTime(tick: number | Date) {
  const d = new Date(typeof tick === 'number' ? tick : tick.getTime())
  if (period.value === '24h') {
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  }
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' })
}

function formatDollars(tick: number | Date) {
  const v = typeof tick === 'number' ? tick : 0
  if (v >= 1) return `$${v.toFixed(1)}`
  if (v >= 0.01) return `$${v.toFixed(2)}`
  return `$${v.toFixed(3)}`
}

function formatTokenCount(tick: number | Date) {
  const v = typeof tick === 'number' ? tick : 0
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`
  if (v >= 1_000) return `${(v / 1_000).toFixed(0)}K`
  return v.toFixed(0)
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toLocaleString()
}

const categoryColors = [
  '#22c55e', // green
  '#3b82f6', // blue
  '#f59e0b', // amber
  '#ef4444', // red
  '#8b5cf6', // violet
  '#ec4899', // pink
  '#06b6d4', // cyan
  '#f97316', // orange
]

// Cost breakdown categories
const modelCostCategories = computed(() =>
  byModel.value.map((m, i) => ({
    label: m.model,
    value: m.costMicrodollars,
    color: categoryColors[i % categoryColors.length]!,
  })),
)

const keyCostCategories = computed(() =>
  byKey.value.map((k, i) => ({
    label: k.keyName ?? k.keyId.slice(0, 8),
    value: k.costMicrodollars,
    color: categoryColors[(i + 3) % categoryColors.length]!,
  })),
)

// Token breakdown categories (total tokens per model/key)
const modelTokenCategories = computed(() =>
  byModel.value.map((m, i) => ({
    label: m.model,
    value: m.inputTokens + m.outputTokens + m.cacheReadTokens + m.cacheWriteTokens,
    color: categoryColors[i % categoryColors.length]!,
  })),
)

const keyTokenCategories = computed(() =>
  byKey.value.map((k, i) => ({
    label: k.keyName ?? k.keyId.slice(0, 8),
    value: k.inputTokens + k.outputTokens + k.cacheReadTokens + k.cacheWriteTokens,
    color: categoryColors[(i + 3) % categoryColors.length]!,
  })),
)

const hasData = computed(
  () => timeseries.value.length > 0 || byModel.value.length > 0 || byKey.value.length > 0,
)

onMounted(() => fetchAll())
</script>

<template>
  <div class="space-y-4">
    <!-- Header: period selector + clear button -->
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-1">
        <UButton
          v-for="p in periods"
          :key="p.value"
          :color="period === p.value ? 'primary' : 'neutral'"
          :variant="period === p.value ? 'solid' : 'ghost'"
          size="sm"
          :loading="isLoading && period === p.value"
          @click="setPeriod(p.value)"
        >
          {{ p.label }}
        </UButton>
      </div>
      <UButton
        v-if="hasData"
        color="error"
        variant="soft"
        size="sm"
        icon="i-lucide-trash-2"
        @click="clearHistory"
      >
        Clear History
      </UButton>
    </div>

    <!-- Summary cards -->
    <div class="grid grid-cols-3 gap-3">
      <UCard>
        <div class="text-sm text-gray-500 dark:text-gray-400">Total Cost</div>
        <div class="text-xl font-semibold mt-1">{{ formatCost(totalCost) }}</div>
      </UCard>
      <UCard>
        <div class="text-sm text-gray-500 dark:text-gray-400">Total Requests</div>
        <div class="text-xl font-semibold mt-1">{{ totalRequests.toLocaleString() }}</div>
      </UCard>
      <UCard>
        <div class="text-sm text-gray-500 dark:text-gray-400">Avg Cost / Request</div>
        <div class="text-xl font-semibold mt-1">{{ formatCost(avgCostPerRequest) }}</div>
      </UCard>
    </div>

    <!-- Token summary cards -->
    <div v-if="totalTokens > 0" class="grid grid-cols-2 sm:grid-cols-4 gap-3">
      <UCard>
        <div class="text-sm text-gray-500 dark:text-gray-400">Input Tokens</div>
        <div class="text-lg font-semibold mt-1">{{ formatTokens(totalInputTokens) }}</div>
      </UCard>
      <UCard>
        <div class="text-sm text-gray-500 dark:text-gray-400">Output Tokens</div>
        <div class="text-lg font-semibold mt-1">{{ formatTokens(totalOutputTokens) }}</div>
      </UCard>
      <UCard>
        <div class="text-sm text-gray-500 dark:text-gray-400">Cache Read</div>
        <div class="text-lg font-semibold mt-1">{{ formatTokens(totalCacheReadTokens) }}</div>
      </UCard>
      <UCard>
        <div class="text-sm text-gray-500 dark:text-gray-400">Cache Write</div>
        <div class="text-lg font-semibold mt-1">{{ formatTokens(totalCacheWriteTokens) }}</div>
      </UCard>
    </div>

    <!-- Empty state -->
    <UAlert
      v-if="!hasData && !isLoading"
      color="neutral"
      title="No usage data"
      description="Usage history will appear here after proxy requests are made."
      icon="i-lucide-chart-area"
    />

    <!-- Area chart: cost over time -->
    <UCard v-if="areaData.length > 0">
      <template #header>
        <span class="text-sm font-medium">Cost over time</span>
      </template>
      <AreaChart
        :data="areaData"
        :height="250"
        :categories="areaCategories"
        :x-formatter="formatTime"
        :y-formatter="formatDollars"
        :y-grid-line="true"
        :hide-legend="true"
        :x-num-ticks="6"
        :y-num-ticks="4"
      />
    </UCard>

    <!-- Area chart: tokens over time -->
    <UCard v-if="tokenAreaData.length > 0 && totalTokens > 0">
      <template #header>
        <span class="text-sm font-medium">Tokens over time</span>
      </template>
      <AreaChart
        :data="tokenAreaData"
        :height="250"
        :categories="tokenAreaCategories"
        :x-formatter="formatTime"
        :y-formatter="formatTokenCount"
        :y-grid-line="true"
        :x-num-ticks="6"
        :y-num-ticks="4"
      />
    </UCard>

    <!-- Breakdown: cost by model + key -->
    <div v-if="hasData" class="grid grid-cols-1 sm:grid-cols-2 gap-4">
      <UCard v-if="modelCostCategories.length > 0">
        <template #header>
          <span class="text-sm font-medium">Cost by model</span>
        </template>
        <CategoryDistribution
          :primary-value="formatCost(totalCost)"
          :categories="modelCostCategories"
          :gap="2"
        />
      </UCard>

      <UCard v-if="keyCostCategories.length > 0">
        <template #header>
          <span class="text-sm font-medium">Cost by key</span>
        </template>
        <CategoryDistribution
          :primary-value="formatCost(totalCost)"
          :categories="keyCostCategories"
          :gap="2"
        />
      </UCard>
    </div>

    <!-- Breakdown: tokens by model + key -->
    <div v-if="hasData && totalTokens > 0" class="grid grid-cols-1 sm:grid-cols-2 gap-4">
      <UCard v-if="modelTokenCategories.length > 0">
        <template #header>
          <span class="text-sm font-medium">Tokens by model</span>
        </template>
        <CategoryDistribution
          :primary-value="formatTokens(totalTokens)"
          :categories="modelTokenCategories"
          :gap="2"
        />
      </UCard>

      <UCard v-if="keyTokenCategories.length > 0">
        <template #header>
          <span class="text-sm font-medium">Tokens by key</span>
        </template>
        <CategoryDistribution
          :primary-value="formatTokens(totalTokens)"
          :categories="keyTokenCategories"
          :gap="2"
        />
      </UCard>
    </div>
  </div>
</template>

<style scoped>
:deep() {
  --vis-dark-axis-grid-color: rgba(108, 119, 140, 0.25);
  --vis-axis-grid-color: rgba(200, 200, 210, 0.3);
}
</style>
