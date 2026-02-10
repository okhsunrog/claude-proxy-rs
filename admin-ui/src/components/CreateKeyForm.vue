<script setup lang="ts">
import { ref } from 'vue'
import { errorMessage } from '../utils/format'

const emit = defineEmits<{
  created: []
}>()

const props = defineProps<{
  createKey: (name: string) => Promise<boolean>
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
  </div>
</template>
