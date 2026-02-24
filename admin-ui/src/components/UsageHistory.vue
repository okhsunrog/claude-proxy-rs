<script setup lang="ts">
import { onMounted, computed } from 'vue'
import { AreaChart, DonutChart } from 'vue-chrts'
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
  fetchAll,
  clearHistory,
  setPeriod,
} = useUsageHistory()

const periods: { label: string; value: Period }[] = [
  { label: '24h', value: '24h' },
  { label: '7d', value: '7d' },
  { label: '30d', value: '30d' },
]

// Area chart data: transform timeseries into { x, cost } format
const areaData = computed(() =>
  timeseries.value.map((p) => ({
    x: p.timestamp,
    cost: p.costMicrodollars / 1_000_000,
  })),
)

const areaCategories = computed(() => ({
  cost: { name: 'Cost ($)' },
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

// Donut chart: model breakdown
const modelDonutData = computed(() => byModel.value.map((m) => m.costMicrodollars))

const modelColors = [
  '#22c55e', // green
  '#3b82f6', // blue
  '#f59e0b', // amber
  '#ef4444', // red
  '#8b5cf6', // violet
  '#ec4899', // pink
  '#06b6d4', // cyan
  '#f97316', // orange
]

const modelDonutCategories = computed(() => {
  const cats: Record<string, { name: string; color: string }> = {}
  byModel.value.forEach((m, i) => {
    cats[`${i}`] = {
      name: `${m.model} (${formatCost(m.costMicrodollars)})`,
      color: modelColors[i % modelColors.length]!,
    }
  })
  return cats
})

// Donut chart: key breakdown
const keyDonutData = computed(() => byKey.value.map((k) => k.costMicrodollars))

const keyDonutCategories = computed(() => {
  const cats: Record<string, { name: string; color: string }> = {}
  byKey.value.forEach((k, i) => {
    cats[`${i}`] = {
      name: `${k.keyName ?? k.keyId.slice(0, 8)} (${formatCost(k.costMicrodollars)})`,
      color: modelColors[(i + 3) % modelColors.length]!,
    }
  })
  return cats
})

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

    <!-- Empty state -->
    <UAlert
      v-if="!hasData && !isLoading"
      color="neutral"
      title="No usage data"
      description="Usage history will appear here after proxy requests are made."
      icon="i-lucide-chart-area"
    />

    <!-- Area chart: cost over time -->
    <UCard v-if="areaData.length > 1">
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

    <!-- Breakdown: model + key donuts -->
    <div v-if="hasData" class="grid grid-cols-1 sm:grid-cols-2 gap-4">
      <UCard v-if="modelDonutData.length > 0">
        <template #header>
          <span class="text-sm font-medium">Cost by model</span>
        </template>
        <div class="flex justify-center">
          <DonutChart
            :data="modelDonutData"
            :radius="90"
            :arc-width="30"
            :categories="modelDonutCategories"
          />
        </div>
      </UCard>

      <UCard v-if="keyDonutData.length > 0">
        <template #header>
          <span class="text-sm font-medium">Cost by key</span>
        </template>
        <div class="flex justify-center">
          <DonutChart
            :data="keyDonutData"
            :radius="90"
            :arc-width="30"
            :categories="keyDonutCategories"
          />
        </div>
      </UCard>
    </div>
  </div>
</template>
