<script setup lang="ts">
import { onMounted, computed } from 'vue'
import { Line } from 'vue-chartjs'
import {
  Chart as ChartJS,
  LineElement,
  PointElement,
  LinearScale,
  CategoryScale,
  Filler,
  Tooltip,
  Legend,
  type TooltipItem,
} from 'chart.js'
import CategoryDistribution from './CategoryDistribution.vue'
import { useUsageHistory, type Period } from '../composables/useUsageHistory'
import { formatCost } from '../utils/format'

ChartJS.register(LineElement, PointElement, LinearScale, CategoryScale, Filler, Tooltip, Legend)

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

function formatTimeLabel(timestamp: number): string {
  const d = new Date(timestamp)
  if (period.value === '24h') {
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  }
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' })
}

function formatDollars(v: number): string {
  if (v >= 1) return `$${v.toFixed(1)}`
  if (v >= 0.01) return `$${v.toFixed(2)}`
  return `$${v.toFixed(3)}`
}

function formatTokenCount(v: number): string {
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`
  if (v >= 1_000) return `${(v / 1_000).toFixed(0)}K`
  return v.toFixed(0)
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toLocaleString()
}

const isDark = computed(() => document.documentElement.classList.contains('dark'))

const gridColor = computed(() =>
  isDark.value ? 'rgba(108, 119, 140, 0.25)' : 'rgba(200, 200, 210, 0.3)',
)

const baseChartOptions = computed(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: { intersect: false, mode: 'index' as const },
  plugins: {
    legend: { display: false },
  },
  scales: {
    x: {
      grid: { color: gridColor.value },
      ticks: {
        color: isDark.value ? '#9ca3af' : '#6b7280',
        maxTicksLimit: 6,
      },
    },
    y: {
      grid: { color: gridColor.value },
      ticks: {
        color: isDark.value ? '#9ca3af' : '#6b7280',
        maxTicksLimit: 4,
      },
    },
  },
  elements: {
    point: { radius: 0, hoverRadius: 4 },
    line: { tension: 0.3 },
  },
}))

// Cost chart
const costChartData = computed(() => ({
  labels: timeseries.value.map((p) => formatTimeLabel(p.timestamp)),
  datasets: [
    {
      label: 'Cost ($)',
      data: timeseries.value.map((p) => p.costMicrodollars / 1_000_000),
      borderColor: '#22c55e',
      backgroundColor: 'rgba(34, 197, 94, 0.15)',
      fill: true,
    },
  ],
}))

const costChartOptions = computed(() => ({
  ...baseChartOptions.value,
  plugins: {
    ...baseChartOptions.value.plugins,
    tooltip: {
      callbacks: {
        label: (ctx: TooltipItem<'line'>) => formatDollars(ctx.parsed.y ?? 0),
      },
    },
  },
  scales: {
    ...baseChartOptions.value.scales,
    y: {
      ...baseChartOptions.value.scales.y,
      ticks: {
        ...baseChartOptions.value.scales.y.ticks,
        callback: (v: string | number) => formatDollars(Number(v)),
      },
    },
  },
}))

// Token chart
const tokenChartData = computed(() => ({
  labels: timeseries.value.map((p) => formatTimeLabel(p.timestamp)),
  datasets: [
    {
      label: 'Input',
      data: timeseries.value.map((p) => p.inputTokens),
      borderColor: '#3b82f6',
      backgroundColor: 'rgba(59, 130, 246, 0.1)',
      fill: true,
    },
    {
      label: 'Output',
      data: timeseries.value.map((p) => p.outputTokens),
      borderColor: '#22c55e',
      backgroundColor: 'rgba(34, 197, 94, 0.1)',
      fill: true,
    },
    {
      label: 'Cache Read',
      data: timeseries.value.map((p) => p.cacheReadTokens),
      borderColor: '#f59e0b',
      backgroundColor: 'rgba(245, 158, 11, 0.1)',
      fill: true,
    },
    {
      label: 'Cache Write',
      data: timeseries.value.map((p) => p.cacheWriteTokens),
      borderColor: '#8b5cf6',
      backgroundColor: 'rgba(139, 92, 246, 0.1)',
      fill: true,
    },
  ],
}))

const tokenChartOptions = computed(() => ({
  ...baseChartOptions.value,
  plugins: {
    ...baseChartOptions.value.plugins,
    legend: {
      display: true,
      labels: {
        color: isDark.value ? '#9ca3af' : '#6b7280',
        usePointStyle: true,
        pointStyle: 'circle',
      },
    },
    tooltip: {
      callbacks: {
        label: (ctx: TooltipItem<'line'>) =>
          `${ctx.dataset.label ?? ''}: ${formatTokenCount(ctx.parsed.y ?? 0)}`,
      },
    },
  },
  scales: {
    ...baseChartOptions.value.scales,
    y: {
      ...baseChartOptions.value.scales.y,
      ticks: {
        ...baseChartOptions.value.scales.y.ticks,
        callback: (v: string | number) => formatTokenCount(Number(v)),
      },
    },
  },
}))

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
    <UCard v-if="timeseries.length > 0">
      <template #header>
        <span class="text-sm font-medium">Cost over time</span>
      </template>
      <div style="height: 250px">
        <Line :data="costChartData" :options="costChartOptions" />
      </div>
    </UCard>

    <!-- Area chart: tokens over time -->
    <UCard v-if="timeseries.length > 0 && totalTokens > 0">
      <template #header>
        <span class="text-sm font-medium">Tokens over time</span>
      </template>
      <div style="height: 250px">
        <Line :data="tokenChartData" :options="tokenChartOptions" />
      </div>
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
          :interactive="true"
          :format-value="(v: number) => formatCost(v)"
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
          :interactive="true"
          :format-value="(v: number) => formatTokens(v)"
        />
      </UCard>
    </div>
  </div>
</template>
