<script setup lang="ts">
import { ref, computed } from 'vue'

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
    interactive?: boolean
    formatValue?: (value: number) => string
  }>(),
  { gap: 2, interactive: false },
)

const excluded = ref(new Set<string>())

function toggle(label: string) {
  if (!props.interactive) return
  const next = new Set(excluded.value)
  if (next.has(label)) {
    next.delete(label)
  } else {
    // Don't allow excluding all items
    if (next.size >= props.categories.length - 1) return
    next.add(label)
  }
  excluded.value = next
}

const activeCategories = computed(() =>
  props.interactive
    ? props.categories.filter((c) => !excluded.value.has(c.label))
    : props.categories,
)

const activeTotal = computed(() => activeCategories.value.reduce((sum, c) => sum + c.value, 0))

const segments = computed(() =>
  props.categories.map((c) => {
    const isExcluded = excluded.value.has(c.label)
    return {
      ...c,
      isExcluded,
      percentage: !isExcluded && activeTotal.value > 0 ? (c.value / activeTotal.value) * 100 : 0,
    }
  }),
)

const activePrimaryValue = computed(() => {
  if (!props.interactive || excluded.value.size === 0) return props.primaryValue
  if (props.formatValue) return props.formatValue(activeTotal.value)
  return props.primaryValue
})
</script>

<template>
  <div class="w-full">
    <div v-if="activePrimaryValue" class="text-xl font-semibold mb-3">{{ activePrimaryValue }}</div>
    <div class="flex w-full h-3 rounded-full overflow-hidden" :style="{ gap: `${gap}px` }">
      <div
        v-for="(seg, i) in segments.filter((s) => !s.isExcluded)"
        :key="i"
        class="h-full rounded-full transition-all duration-300"
        :style="{ width: `${seg.percentage}%`, backgroundColor: seg.color, minWidth: seg.percentage > 0 ? '4px' : '0' }"
      />
    </div>
    <div class="mt-3 flex flex-wrap gap-x-4 gap-y-1.5">
      <div
        v-for="(seg, i) in segments"
        :key="i"
        class="flex items-center gap-1.5 text-sm transition-opacity duration-200"
        :class="[
          interactive ? 'cursor-pointer select-none' : '',
          seg.isExcluded ? 'opacity-40' : '',
        ]"
        @click="toggle(seg.label)"
      >
        <span
          class="w-2.5 h-2.5 rounded-full shrink-0 transition-all duration-200"
          :style="{ backgroundColor: seg.isExcluded ? '#6b7280' : seg.color }"
        />
        <span
          class="truncate"
          :class="seg.isExcluded ? 'line-through text-gray-400 dark:text-gray-500' : 'text-gray-500 dark:text-gray-400'"
        >{{ seg.label }}</span>
        <span v-if="!seg.isExcluded" class="font-medium">{{ seg.percentage.toFixed(1) }}%</span>
      </div>
    </div>
  </div>
</template>
