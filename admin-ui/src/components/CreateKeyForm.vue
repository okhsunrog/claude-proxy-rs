<script setup lang="ts">
import { ref } from 'vue'
import { errorMessage } from '../utils/format'
import type { CreateKeyResponse } from '../client'

const emit = defineEmits<{
  created: []
}>()

const props = defineProps<{
  createKey: (name: string) => Promise<boolean>
  newKeyData: CreateKeyResponse | null
}>()

const toast = useToast()
const keyName = ref('')
const isCreating = ref(false)

async function handleCreate() {
  const name = keyName.value.trim()
  if (!name) {
    toast.add({ title: 'Please enter a key name', color: 'warning' })
    return
  }

  isCreating.value = true
  try {
    await props.createKey(name)
    keyName.value = ''
    toast.add({ title: 'API key created successfully', color: 'success' })
    emit('created')
  } catch (e: unknown) {
    toast.add({ title: 'Failed to create key', description: errorMessage(e), color: 'error' })
  } finally {
    isCreating.value = false
  }
}

function copyNewKey() {
  if (!props.newKeyData?.key) return
  navigator.clipboard.writeText(props.newKeyData.key).then(
    () => toast.add({ title: 'Key copied to clipboard', color: 'success' }),
    () => toast.add({ title: 'Failed to copy key', color: 'error' }),
  )
}
</script>

<template>
  <div class="space-y-3">
    <label class="block font-semibold text-sm">Create New API Key</label>
    <div class="flex gap-3">
      <UInput
        v-model="keyName"
        placeholder="Key name (e.g., 'My App')"
        class="flex-1"
        @keyup.enter="handleCreate"
      />
      <UButton color="primary" :loading="isCreating" @click="handleCreate">
        Generate Key
      </UButton>
    </div>

    <UAlert
      v-if="newKeyData"
      color="success"
      title="API Key Created"
    >
      <div class="flex items-center gap-2 mt-2">
        <code class="flex-1 break-all text-sm font-mono">{{ newKeyData.key }}</code>
        <UButton size="xs" variant="soft" @click="copyNewKey">Copy</UButton>
      </div>
    </UAlert>
  </div>
</template>
