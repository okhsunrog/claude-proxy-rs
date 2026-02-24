<script setup lang="ts">
import { computed } from 'vue'

export interface CategoryItem {
  label: string
  value: number
  color: string
}

const props = withDefaults(
  defineProps<{
    primaryValue?: string
    categories: CategoryItem[]
    gap?: number
  }>(),
  { gap: 2 },
)

const total = computed(() => props.categories.reduce((sum, c) => sum + c.value, 0))

const segments = computed(() =>
  props.categories.map((c) => ({
    ...c,
    percentage: total.value > 0 ? (c.value / total.value) * 100 : 0,
  })),
)
</script>

<template>
  <div class="w-full">
    <div v-if="primaryValue" class="text-xl font-semibold mb-3">{{ primaryValue }}</div>
    <div class="flex w-full h-3 rounded-full overflow-hidden" :style="{ gap: `${gap}px` }">
      <div
        v-for="(seg, i) in segments"
        :key="i"
        class="h-full rounded-full transition-all duration-300"
        :style="{ width: `${seg.percentage}%`, backgroundColor: seg.color, minWidth: seg.percentage > 0 ? '4px' : '0' }"
      />
    </div>
    <div class="mt-3 flex flex-wrap gap-x-4 gap-y-1.5">
      <div v-for="(seg, i) in segments" :key="i" class="flex items-center gap-1.5 text-sm">
        <span class="w-2.5 h-2.5 rounded-full shrink-0" :style="{ backgroundColor: seg.color }" />
        <span class="text-gray-500 dark:text-gray-400 truncate">{{ seg.label }}</span>
        <span class="font-medium">{{ seg.percentage.toFixed(1) }}%</span>
      </div>
    </div>
  </div>
</template>
