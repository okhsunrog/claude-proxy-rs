<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
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
import { useUserUsage, type Period } from '../composables/useUserUsage'
import UsageStats from '../components/UsageStats.vue'
import CategoryDistribution from '../components/CategoryDistribution.vue'
import { formatCost } from '../utils/format'

ChartJS.register(LineElement, PointElement, LinearScale, CategoryScale, Filler, Tooltip, Legend)

const {
  token,
  isAuthenticated,
  usage,
  timeseries,
  byModel,
  period,
  isLoading,
  error,
  saveToken,
  clearToken,
  fetchAll,
  setPeriod,
} = useUserUsage()

const tokenInput = ref('')
const activeTab = ref<'cost' | 'tokens'>('cost')
const periods: { label: string; value: Period }[] = [
  { label: '24h', value: '24h' },
  { label: '7d', value: '7d' },
  { label: '30d', value: '30d' },
]

onMounted(async () => {
  if (isAuthenticated.value) await fetchAll()
})

async function submitToken() {
  if (!tokenInput.value.trim()) return
  saveToken(tokenInput.value)
  tokenInput.value = ''
  await fetchAll()
}

// Usage stats shape expected by UsageStats component
const keyUsage = computed(() => {
  if (!usage.value) return undefined
  return { limits: usage.value.limits, usage: usage.value.usage }
})

// Aggregate token counts per window from model_entries
const windowTokens = computed(() => {
  const entries = usage.value?.modelEntries ?? []
  const sum = (fn: (e: (typeof entries)[0]) => number) => entries.reduce((s, e) => s + fn(e), 0)
  return {
    fiveHour: sum(
      (e) => e.fiveHour.input + e.fiveHour.output + e.fiveHour.cacheRead + e.fiveHour.cacheWrite,
    ),
    weekly: sum((e) => e.weekly.input + e.weekly.output + e.weekly.cacheRead + e.weekly.cacheWrite),
    total: sum((e) => e.total.input + e.total.output + e.total.cacheRead + e.total.cacheWrite),
  }
})

// Chart helpers
const isDark = computed(() => document.documentElement.classList.contains('dark'))
const gridColor = computed(() =>
  isDark.value ? 'rgba(108, 119, 140, 0.25)' : 'rgba(200, 200, 210, 0.3)',
)

function formatTimeLabel(timestamp: number): string {
  const d = new Date(timestamp)
  if (period.value === '24h')
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' })
}

function formatDollars(v: number): string {
  if (v >= 1) return `$${v.toFixed(2)}`
  if (v >= 0.01) return `$${v.toFixed(3)}`
  return `$${v.toFixed(4)}`
}

function formatTokenCount(v: number): string {
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`
  if (v >= 1_000) return `${(v / 1_000).toFixed(0)}K`
  return v.toFixed(0)
}

const baseChartOptions = computed(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: { intersect: false, mode: 'index' as const },
  plugins: { legend: { display: false } },
  scales: {
    x: {
      grid: { color: gridColor.value },
      ticks: { color: isDark.value ? '#9ca3af' : '#6b7280', maxTicksLimit: 6 },
    },
    y: {
      grid: { color: gridColor.value },
      ticks: { color: isDark.value ? '#9ca3af' : '#6b7280', maxTicksLimit: 4 },
    },
  },
  elements: { point: { radius: 0, hoverRadius: 4 }, line: { tension: 0.3 } },
}))

const points = computed(() => timeseries.value?.points ?? [])

const costChartData = computed(() => ({
  labels: points.value.map((p) => formatTimeLabel(p.timestamp)),
  datasets: [
    {
      label: 'Cost ($)',
      data: points.value.map((p) => p.costMicrodollars / 1_000_000),
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
      callbacks: { label: (ctx: TooltipItem<'line'>) => formatDollars(ctx.parsed.y ?? 0) },
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

const tokenChartData = computed(() => ({
  labels: points.value.map((p) => formatTimeLabel(p.timestamp)),
  datasets: [
    {
      label: 'Input',
      data: points.value.map((p) => p.inputTokens),
      borderColor: '#3b82f6',
      backgroundColor: 'rgba(59, 130, 246, 0.1)',
      fill: true,
    },
    {
      label: 'Output',
      data: points.value.map((p) => p.outputTokens),
      borderColor: '#22c55e',
      backgroundColor: 'rgba(34, 197, 94, 0.1)',
      fill: true,
    },
    {
      label: 'Cache Read',
      data: points.value.map((p) => p.cacheReadTokens),
      borderColor: '#f59e0b',
      backgroundColor: 'rgba(245, 158, 11, 0.1)',
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
  '#22c55e',
  '#3b82f6',
  '#f59e0b',
  '#ef4444',
  '#8b5cf6',
  '#ec4899',
  '#06b6d4',
  '#f97316',
]

const totalCost = computed(() => points.value.reduce((s, p) => s + p.costMicrodollars, 0))

const totalTokens = computed(() =>
  points.value.reduce(
    (s, p) => s + p.inputTokens + p.outputTokens + p.cacheReadTokens + p.cacheWriteTokens,
    0,
  ),
)

const modelCategories = computed(() =>
  (byModel.value?.models ?? []).map((m, i) => ({
    label: m.model,
    value: m.costMicrodollars,
    color: categoryColors[i % categoryColors.length]!,
  })),
)

const modelTokenCategories = computed(() =>
  (byModel.value?.models ?? []).map((m, i) => ({
    label: m.model,
    value: m.inputTokens + m.outputTokens + m.cacheReadTokens + m.cacheWriteTokens,
    color: categoryColors[i % categoryColors.length]!,
  })),
)

const hasHistory = computed(() => points.value.some((p) => p.requestCount > 0))
</script>

<template>
  <div class="min-h-screen bg-background">
    <div class="max-w-4xl mx-auto px-4 py-8 space-y-6">
      <!-- Header -->
      <div class="flex items-center justify-between">
        <div>
          <h1 class="text-2xl font-bold">My Usage</h1>
          <p v-if="usage" class="text-sm text-muted mt-0.5">{{ usage.keyName }}</p>
        </div>
        <UButton
          v-if="isAuthenticated"
          color="neutral"
          variant="ghost"
          size="sm"
          icon="i-lucide-log-out"
          @click="clearToken"
        >
          Change key
        </UButton>
      </div>

      <!-- Token entry form -->
      <UCard v-if="!isAuthenticated">
        <div class="space-y-4">
          <div>
            <h2 class="text-lg font-semibold">Enter your API key</h2>
            <p class="text-sm text-muted mt-1">
              Your key will be saved in browser storage and never sent to any server except this
              proxy.
            </p>
          </div>
          <form class="flex gap-2" @submit.prevent="submitToken">
            <UInput
              v-model="tokenInput"
              class="flex-1 font-mono"
              placeholder="sk-proxy-..."
              type="password"
              autofocus
            />
            <UButton type="submit" :disabled="!tokenInput.trim()"> Connect </UButton>
          </form>
          <UAlert v-if="error" color="error" icon="i-lucide-circle-x" :title="error" />
        </div>
      </UCard>

      <!-- Dashboard -->
      <template v-else>
        <UAlert v-if="error" color="error" icon="i-lucide-circle-x" :title="error" />

        <!-- Usage limits / counters -->
        <UCard>
          <template #header>
            <div class="flex items-center justify-between">
              <span class="text-sm font-medium">Usage &amp; limits</span>
              <UButton
                size="xs"
                variant="ghost"
                icon="i-lucide-refresh-cw"
                :loading="isLoading"
                @click="fetchAll"
              />
            </div>
          </template>
          <UsageStats :usage="keyUsage" />
          <!-- Token counts per window -->
          <div v-if="windowTokens.total > 0" class="grid grid-cols-3 gap-3 mt-3">
            <div
              v-for="item in [
                { label: '5-Hour tokens', value: windowTokens.fiveHour },
                { label: 'Weekly tokens', value: windowTokens.weekly },
                { label: 'Total tokens', value: windowTokens.total },
              ]"
              :key="item.label"
              class="rounded-lg bg-elevated p-3"
            >
              <div class="text-xs text-muted uppercase">{{ item.label }}</div>
              <div class="text-lg font-semibold mt-1">{{ formatTokenCount(item.value) }}</div>
            </div>
          </div>
        </UCard>

        <!-- Period selector -->
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

        <!-- Empty state -->
        <UAlert
          v-if="!isLoading && !hasHistory"
          color="neutral"
          icon="i-lucide-chart-area"
          title="No usage history"
          description="History will appear here once you've made requests through the proxy."
        />

        <!-- Tabbed charts + breakdown -->
        <template v-if="hasHistory">
          <UTabs
            :items="[
              { label: 'Cost', value: 'cost' },
              { label: 'Tokens', value: 'tokens' },
            ]"
            :model-value="activeTab"
            @update:model-value="(v) => (activeTab = v as 'cost' | 'tokens')"
          />

          <!-- Cost tab -->
          <template v-if="activeTab === 'cost'">
            <UCard>
              <template #header><span class="text-sm font-medium">Cost over time</span></template>
              <div style="height: 220px">
                <Line :data="costChartData" :options="costChartOptions" />
              </div>
            </UCard>
            <UCard v-if="modelCategories.length > 0">
              <template #header><span class="text-sm font-medium">Cost by model</span></template>
              <CategoryDistribution
                :primary-value="formatCost(totalCost)"
                :categories="modelCategories"
                :gap="2"
              />
            </UCard>
          </template>

          <!-- Tokens tab -->
          <template v-if="activeTab === 'tokens'">
            <UCard>
              <template #header><span class="text-sm font-medium">Tokens over time</span></template>
              <div style="height: 220px">
                <Line :data="tokenChartData" :options="tokenChartOptions" />
              </div>
            </UCard>
            <UCard v-if="modelTokenCategories.length > 0">
              <template #header><span class="text-sm font-medium">Tokens by model</span></template>
              <CategoryDistribution
                :primary-value="formatTokenCount(totalTokens)"
                :categories="modelTokenCategories"
                :gap="2"
                :format-value="(v: number) => formatTokenCount(v)"
              />
            </UCard>
          </template>
        </template>
      </template>
    </div>
  </div>
</template>
