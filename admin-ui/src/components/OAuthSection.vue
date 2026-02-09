<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useOAuth } from '../composables/useOAuth'

const { isConnected, isLoading, error, showCodeInput, checkStatus, connect, exchangeCode, disconnect } = useOAuth()
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
</script>

<template>
  <UCard>
    <template #header>
      <h2 class="text-xl font-semibold">OAuth Connection</h2>
    </template>

    <div class="space-y-4">
      <div class="flex items-center gap-3">
        <span class="text-sm">Status:</span>
        <UBadge v-if="isConnected" color="success" variant="subtle">Connected</UBadge>
        <UBadge v-else color="error" variant="subtle">Not connected</UBadge>
      </div>

      <div class="flex gap-2">
        <UButton
          v-if="!isConnected"
          color="primary"
          :loading="isLoading"
          @click="connect"
        >
          Connect with Claude
        </UButton>
        <UButton
          v-else
          color="error"
          variant="soft"
          :loading="isLoading"
          @click="showDisconnectModal = true"
        >
          Disconnect
        </UButton>
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
