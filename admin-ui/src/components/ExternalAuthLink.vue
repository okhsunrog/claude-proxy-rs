<script setup lang="ts">
defineProps<{ url: string; label: string }>()
const toast = useToast()

async function copy(url: string) {
  try {
    await navigator.clipboard.writeText(url)
    toast.add({ title: 'Link copied', color: 'success' })
  } catch {
    toast.add({
      title: 'Unable to copy automatically',
      description: 'Select the link above and copy it manually.',
      color: 'error',
    })
  }
}
</script>

<template>
  <div class="space-y-2">
    <p class="text-sm text-muted">Open this link in the browser where you want to sign in:</p>
    <UInput :model-value="url" :aria-label="label" readonly class="w-full" />
    <div class="flex flex-wrap gap-2">
      <UButton :to="url" target="_blank" rel="noopener noreferrer" icon="i-lucide-external-link">
        Open
      </UButton>
      <UButton color="neutral" variant="outline" icon="i-lucide-copy" @click="copy(url)">
        Copy link
      </UButton>
    </div>
  </div>
</template>
