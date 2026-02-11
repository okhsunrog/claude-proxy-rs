<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useOAuth } from '../composables/useOAuth'

const {
  isConnected,
  isLoading,
  isLoadingUsage,
  error,
  showCodeInput,
  subscriptionUsage,
  planName,
  checkStatus,
  connect,
  exchangeCode,
  disconnect,
  loadUsage,
} = useOAuth()
const toast = useToast()
const oauthCode = ref('')
const showDisconnectModal = ref(false)

onMounted(() => {
  checkStatus()
})

async function handleExchangeCode() {
  const success = await exchangeCode(oauthCode.value)
  if (success) {
    oauthCode.value = ''
    toast.add({ title: 'Successfully connected!', color: 'success' })
  }
}

async function handleDisconnect() {
  showDisconnectModal.value = false
  const success = await disconnect()
  if (success) {
    toast.add({ title: 'Disconnected successfully', color: 'success' })
  }
}

function barColor(pct: number): string {
  if (pct >= 90) return 'error'
  if (pct >= 70) return 'warning'
  return 'success'
}

function formatResetTime(isoString: string): string {
  const reset = new Date(isoString)
  const now = new Date()
  const diffMs = reset.getTime() - now.getTime()
  if (diffMs <= 0) return 'Resetting...'
  const hours = Math.floor(diffMs / 3600000)
  const minutes = Math.floor((diffMs % 3600000) / 60000)
  const abs = `${String(reset.getDate()).padStart(2, '0')}/${String(reset.getMonth() + 1).padStart(2, '0')}, ${String(reset.getHours()).padStart(2, '0')}:${String(reset.getMinutes()).padStart(2, '0')}`
  if (hours >= 24) {
    const days = Math.floor(hours / 24)
    const remainingHours = hours % 24
    return `Resets in ${days}d ${remainingHours}h (${abs})`
  }
  return `Resets in ${hours}h ${minutes}m (${abs})`
}

interface UsageDisplayItem {
  label: string
  utilization: number
  resetsAt?: string | null
  subtitle?: string
}

function getUsageItems(): UsageDisplayItem[] {
  if (!subscriptionUsage.value) return []
  const items: UsageDisplayItem[] = []
  const u = subscriptionUsage.value

  if (u.five_hour?.utilization != null) {
    items.push({
      label: 'Session (5h)',
      utilization: u.five_hour.utilization,
      resetsAt: u.five_hour.resets_at,
    })
  }
  if (u.seven_day?.utilization != null) {
    items.push({
      label: 'Weekly (all)',
      utilization: u.seven_day.utilization,
      resetsAt: u.seven_day.resets_at,
    })
  }
  if (u.seven_day_opus?.utilization != null) {
    items.push({
      label: 'Weekly (Opus)',
      utilization: u.seven_day_opus.utilization,
      resetsAt: u.seven_day_opus.resets_at,
    })
  }
  if (u.seven_day_sonnet?.utilization != null) {
    items.push({
      label: 'Weekly (Sonnet)',
      utilization: u.seven_day_sonnet.utilization,
      resetsAt: u.seven_day_sonnet.resets_at,
    })
  }
  if (u.extra_usage) {
    const e = u.extra_usage
    if (e.is_enabled && e.utilization != null) {
      const spent = e.used_credits != null ? `$${(e.used_credits / 100).toFixed(2)}` : null
      const limit = e.monthly_limit != null ? `$${(e.monthly_limit / 100).toFixed(2)}` : null
      items.push({
        label: 'Extra usage',
        utilization: e.utilization,
        subtitle: spent && limit ? `${spent} / ${limit} spent` : undefined,
      })
    }
  }
  return items
}
</script>

<template>
  <UCard>
    <template #header>
      <div class="flex items-center justify-between">
        <h2 class="text-xl font-semibold">Claude Subscription</h2>
        <UButton
          v-if="isConnected && subscriptionUsage"
          size="xs"
          variant="ghost"
          icon="i-lucide-refresh-cw"
          :loading="isLoadingUsage"
          @click="loadUsage"
        />
      </div>
    </template>

    <div class="space-y-4">
      <!-- Status row: badge + plan + connect/disconnect button all inline -->
      <div class="flex items-center gap-3">
        <span class="text-sm">Status:</span>
        <UBadge v-if="isConnected" color="success" variant="subtle">Connected</UBadge>
        <UBadge v-else color="error" variant="subtle">Not connected</UBadge>
        <UBadge v-if="planName" color="info" variant="subtle">{{ planName }}</UBadge>
        <UButton
          v-if="!isConnected"
          size="xs"
          color="primary"
          :loading="isLoading"
          @click="connect"
        >
          Connect with Claude
        </UButton>
        <UButton
          v-else
          size="xs"
          color="error"
          variant="soft"
          :loading="isLoading"
          @click="showDisconnectModal = true"
        >
          Disconnect
        </UButton>
      </div>

      <!-- Subscription Limits -->
      <div v-if="isConnected && subscriptionUsage && getUsageItems().length > 0">
        <div class="text-xs text-muted uppercase mb-2">Subscription Limits</div>
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div
            v-for="item in getUsageItems()"
            :key="item.label"
            class="rounded-lg bg-elevated p-3"
          >
            <div class="text-xs text-muted uppercase">{{ item.label }}</div>
            <div class="text-lg font-semibold mt-1">{{ Math.floor(item.utilization) }}%</div>
            <UProgress
              :model-value="Math.min(item.utilization, 100)"
              :color="barColor(item.utilization)"
              size="xs"
              class="mt-1.5"
            />
            <div v-if="item.resetsAt" class="text-xs text-muted mt-1">
              {{ formatResetTime(item.resetsAt) }}
            </div>
            <div v-if="item.subtitle" class="text-xs text-muted mt-1">
              {{ item.subtitle }}
            </div>
          </div>
        </div>
      </div>

      <div v-if="isConnected && isLoadingUsage && !subscriptionUsage" class="text-sm text-muted">
        Loading usage...
      </div>

      <div v-if="showCodeInput" class="space-y-2">
        <p class="text-sm text-muted">Paste the authorization code from the popup:</p>
        <div class="flex gap-2">
          <UInput
            v-model="oauthCode"
            placeholder="Authorization code"
            class="flex-1"
            @keyup.enter="handleExchangeCode"
          />
          <UButton color="primary" :loading="isLoading" @click="handleExchangeCode">
            Submit
          </UButton>
        </div>
      </div>

      <UAlert
        v-if="error"
        color="error"
        title="Error"
        :description="error"
        :close="{ onClick: () => { error = null } }"
      />
    </div>

    <UModal v-model:open="showDisconnectModal" title="Confirm Disconnect" :ui="{ width: 'max-w-md' }">
      <template #body>
        <p>Are you sure you want to disconnect?</p>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2">
          <UButton variant="ghost" @click="showDisconnectModal = false">Cancel</UButton>
          <UButton color="error" @click="handleDisconnect">Disconnect</UButton>
        </div>
      </template>
    </UModal>
  </UCard>
</template>
