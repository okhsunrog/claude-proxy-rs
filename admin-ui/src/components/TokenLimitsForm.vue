<script setup lang="ts">
import { ref, watch } from 'vue'
import type { UpdateLimitsRequest } from '../client'
import type { KeyUsageResponse } from '../composables/useKeys'

const props = defineProps<{
  keyId: string
  usage: KeyUsageResponse | undefined
  updateLimits: (id: string, limits: UpdateLimitsRequest) => Promise<void>
  resetUsage: (id: string, type: 'hourly' | 'weekly' | 'total' | 'all') => Promise<void>
}>()

const toast = useToast()
const hourlyLimit = ref<number | null>(null)
const weeklyLimit = ref<number | null>(null)
const totalLimit = ref<number | null>(null)
const isSaving = ref(false)

watch(
  () => props.usage,
  (u) => {
    if (u) {
      hourlyLimit.value = u.limits.hourlyLimit ?? null
      weeklyLimit.value = u.limits.weeklyLimit ?? null
      totalLimit.value = u.limits.totalLimit ?? null
    }
  },
  { immediate: true },
)

async function handleSave() {
  isSaving.value = true
  try {
    await props.updateLimits(props.keyId, {
      hourlyLimit: hourlyLimit.value || null,
      weeklyLimit: weeklyLimit.value || null,
      totalLimit: totalLimit.value || null,
    })
    toast.add({ title: 'Limits saved', color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to save limits', description: (e as Error).message, color: 'error' })
  } finally {
    isSaving.value = false
  }
}

const resetItems = [
  [
    {
      label: 'Reset Hourly',
      onSelect: () => handleReset('hourly'),
    },
    {
      label: 'Reset Weekly',
      onSelect: () => handleReset('weekly'),
    },
    {
      label: 'Reset Total',
      onSelect: () => handleReset('total'),
    },
    {
      label: 'Reset All',
      onSelect: () => handleReset('all'),
    },
  ],
]

async function handleReset(type: 'hourly' | 'weekly' | 'total' | 'all') {
  try {
    await props.resetUsage(props.keyId, type)
    toast.add({ title: `${type.charAt(0).toUpperCase() + type.slice(1)} usage reset`, color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to reset usage', description: (e as Error).message, color: 'error' })
  }
}
</script>

<template>
  <div class="border-t border-default pt-3 mt-3">
    <h3 class="text-sm font-semibold text-muted mb-2">Token Limits</h3>
    <div class="flex flex-wrap gap-3 items-end">
      <UFormField label="Hourly" class="w-32">
        <UInput
          :model-value="hourlyLimit ?? undefined"
          @update:model-value="(v: string | number) => hourlyLimit = v == null || v === '' ? null : Number(v)"
          type="number"
          placeholder="Unlimited"
          :min="0"
          class="no-spinners"
        />
      </UFormField>
      <UFormField label="Weekly" class="w-32">
        <UInput
          :model-value="weeklyLimit ?? undefined"
          @update:model-value="(v: string | number) => weeklyLimit = v == null || v === '' ? null : Number(v)"
          type="number"
          placeholder="Unlimited"
          :min="0"
          class="no-spinners"
        />
      </UFormField>
      <UFormField label="Total" class="w-32">
        <UInput
          :model-value="totalLimit ?? undefined"
          @update:model-value="(v: string | number) => totalLimit = v == null || v === '' ? null : Number(v)"
          type="number"
          placeholder="Unlimited"
          :min="0"
          class="no-spinners"
        />
      </UFormField>
    </div>
    <div class="flex gap-2 mt-3">
      <UButton size="sm" color="primary" :loading="isSaving" @click="handleSave">
        Save Limits
      </UButton>
      <UDropdownMenu :items="resetItems">
        <UButton size="sm" variant="soft">Reset Usage</UButton>
      </UDropdownMenu>
    </div>
  </div>
</template>

<style scoped>
/* Hide number input spinner arrows */
.no-spinners :deep(input[type="number"]::-webkit-inner-spin-button),
.no-spinners :deep(input[type="number"]::-webkit-outer-spin-button) {
  -webkit-appearance: none;
  margin: 0;
}
.no-spinners :deep(input[type="number"]) {
  -moz-appearance: textfield;
}
</style>
