<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { errorMessage } from '../utils/format'
import type { Model, UpdateModelRequest } from '../client'
import { useModels } from '../composables/useModels'

const { models, isLoading, loadModels, addModel, deleteModel, updateModel } = useModels()
const toast = useToast()

// Add model form
const showAddModal = ref(false)
const newModelId = ref('')
const newInputPrice = ref<number>(3)
const newOutputPrice = ref<number>(15)
const newCacheReadPrice = ref<number>(0.3)
const newCacheWritePrice = ref<number>(3.75)
const isAdding = ref(false)

// Edit model state
const showEditModal = ref(false)
const editModel = ref<Model | null>(null)
const editInputPrice = ref<number>(0)
const editOutputPrice = ref<number>(0)
const editCacheReadPrice = ref<number>(0)
const editCacheWritePrice = ref<number>(0)
const isSavingEdit = ref(false)

// Delete confirmation
const showDeleteModal = ref(false)
const deleteTarget = ref<Model | null>(null)
const isDeleting = ref(false)

onMounted(() => {
  loadModels()
})

function formatPrice(price: number): string {
  if (price >= 1) return `$${price}`
  return `$${price.toFixed(2)}`
}

async function handleAdd() {
  const id = newModelId.value.trim()
  if (!id) {
    toast.add({ title: 'Please enter a model ID', color: 'warning' })
    return
  }
  isAdding.value = true
  try {
    await addModel({
      id,
      inputPrice: newInputPrice.value,
      outputPrice: newOutputPrice.value,
      cacheReadPrice: newCacheReadPrice.value,
      cacheWritePrice: newCacheWritePrice.value,
    })
    showAddModal.value = false
    newModelId.value = ''
    toast.add({ title: 'Model added', color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to add model', description: errorMessage(e), color: 'error' })
  } finally {
    isAdding.value = false
  }
}

function openEdit(model: Model) {
  editModel.value = model
  editInputPrice.value = model.inputPrice
  editOutputPrice.value = model.outputPrice
  editCacheReadPrice.value = model.cacheReadPrice
  editCacheWritePrice.value = model.cacheWritePrice
  showEditModal.value = true
}

async function handleSaveEdit() {
  if (!editModel.value) return
  isSavingEdit.value = true
  try {
    await updateModel(editModel.value.id, {
      inputPrice: editInputPrice.value,
      outputPrice: editOutputPrice.value,
      cacheReadPrice: editCacheReadPrice.value,
      cacheWritePrice: editCacheWritePrice.value,
    })
    showEditModal.value = false
    toast.add({ title: 'Model updated', color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to update model', description: errorMessage(e), color: 'error' })
  } finally {
    isSavingEdit.value = false
  }
}

async function handleToggleEnabled(model: Model) {
  try {
    const req: UpdateModelRequest = { enabled: !model.enabled }
    await updateModel(model.id, req)
  } catch (e: unknown) {
    toast.add({ title: 'Failed to toggle model', description: errorMessage(e), color: 'error' })
  }
}

function confirmDelete(model: Model) {
  deleteTarget.value = model
  showDeleteModal.value = true
}

async function handleDelete() {
  if (!deleteTarget.value) return
  isDeleting.value = true
  try {
    await deleteModel(deleteTarget.value.id)
    showDeleteModal.value = false
    toast.add({ title: 'Model deleted', color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to delete model', description: errorMessage(e), color: 'error' })
  } finally {
    isDeleting.value = false
  }
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex justify-between items-center">
      <span class="text-sm text-muted">{{ models.length }} models configured</span>
      <UButton size="sm" color="primary" icon="i-lucide-plus" @click="showAddModal = true">
        Add Model
      </UButton>
    </div>

    <div v-if="isLoading && models.length === 0" class="text-center py-8 text-muted">
      Loading...
    </div>

    <div v-else-if="models.length === 0" class="text-center py-8 text-muted">
      No models configured
    </div>

    <div v-else class="space-y-2">
      <div
        v-for="model in models"
        :key="model.id"
        class="rounded-lg border border-default p-3 flex items-center gap-3"
        :class="{ 'opacity-50': !model.enabled }"
      >
        <USwitch
          :model-value="model.enabled"
          @update:model-value="handleToggleEnabled(model)"
          size="sm"
        />
        <div class="flex-1 min-w-0">
          <div class="font-mono text-sm truncate">{{ model.id }}</div>
          <div class="text-xs text-muted mt-0.5">
            In: {{ formatPrice(model.inputPrice) }} |
            Out: {{ formatPrice(model.outputPrice) }} |
            Cache R: {{ formatPrice(model.cacheReadPrice) }} |
            Cache W: {{ formatPrice(model.cacheWritePrice) }}
            <span class="ml-1">($/MTok)</span>
          </div>
        </div>
        <div class="flex gap-1.5">
          <UButton size="xs" variant="ghost" icon="i-lucide-pencil" @click="openEdit(model)" />
          <UButton size="xs" variant="ghost" color="error" icon="i-lucide-trash-2" @click="confirmDelete(model)" />
        </div>
      </div>
    </div>

    <!-- Add Model Modal -->
    <UModal v-model:open="showAddModal" title="Add Model" :ui="{ width: 'max-w-md' }">
      <template #body>
        <div class="space-y-3">
          <UFormField label="Model ID">
            <UInput v-model="newModelId" placeholder="e.g., claude-sonnet-4-5" />
          </UFormField>
          <div class="grid grid-cols-2 gap-3">
            <UFormField label="Input ($/MTok)">
              <UInput v-model.number="newInputPrice" type="number" step="0.01" :min="0" class="no-spinners" />
            </UFormField>
            <UFormField label="Output ($/MTok)">
              <UInput v-model.number="newOutputPrice" type="number" step="0.01" :min="0" class="no-spinners" />
            </UFormField>
            <UFormField label="Cache Read ($/MTok)">
              <UInput v-model.number="newCacheReadPrice" type="number" step="0.01" :min="0" class="no-spinners" />
            </UFormField>
            <UFormField label="Cache Write ($/MTok)">
              <UInput v-model.number="newCacheWritePrice" type="number" step="0.01" :min="0" class="no-spinners" />
            </UFormField>
          </div>
        </div>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2">
          <UButton variant="ghost" @click="showAddModal = false">Cancel</UButton>
          <UButton color="primary" :loading="isAdding" @click="handleAdd">Add Model</UButton>
        </div>
      </template>
    </UModal>

    <!-- Edit Model Modal -->
    <UModal v-model:open="showEditModal" :title="`Edit ${editModel?.id}`" :ui="{ width: 'max-w-md' }">
      <template #body>
        <div class="grid grid-cols-2 gap-3">
          <UFormField label="Input ($/MTok)">
            <UInput v-model.number="editInputPrice" type="number" step="0.01" :min="0" class="no-spinners" />
          </UFormField>
          <UFormField label="Output ($/MTok)">
            <UInput v-model.number="editOutputPrice" type="number" step="0.01" :min="0" class="no-spinners" />
          </UFormField>
          <UFormField label="Cache Read ($/MTok)">
            <UInput v-model.number="editCacheReadPrice" type="number" step="0.01" :min="0" class="no-spinners" />
          </UFormField>
          <UFormField label="Cache Write ($/MTok)">
            <UInput v-model.number="editCacheWritePrice" type="number" step="0.01" :min="0" class="no-spinners" />
          </UFormField>
        </div>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2">
          <UButton variant="ghost" @click="showEditModal = false">Cancel</UButton>
          <UButton color="primary" :loading="isSavingEdit" @click="handleSaveEdit">Save</UButton>
        </div>
      </template>
    </UModal>

    <!-- Delete Confirmation Modal -->
    <UModal v-model:open="showDeleteModal" title="Confirm Delete" :ui="{ width: 'max-w-md' }">
      <template #body>
        <p>Are you sure you want to delete model "<strong>{{ deleteTarget?.id }}</strong>"?</p>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2">
          <UButton variant="ghost" @click="showDeleteModal = false">Cancel</UButton>
          <UButton color="error" :loading="isDeleting" @click="handleDelete">Delete</UButton>
        </div>
      </template>
    </UModal>
  </div>
</template>
