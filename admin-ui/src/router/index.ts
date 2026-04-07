import { createRouter, createWebHistory } from 'vue-router'
import AdminView from '../views/AdminView.vue'
import LoginView from '../views/LoginView.vue'
import UsageView from '../views/UsageView.vue'
import { useAuth } from '../composables/useAuth'

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    {
      path: '/',
      name: 'admin',
      component: AdminView,
      meta: { requiresAuth: true },
    },
    {
      path: '/login',
      name: 'login',
      component: LoginView,
    },
    {
      path: '/usage',
      name: 'usage',
      component: UsageView,
    },
  ],
})

router.beforeEach(async (to) => {
  if (!to.meta.requiresAuth) return true

  const { isAuthenticated, authRequired, checkAuth } = useAuth()

  await checkAuth()

  if (!authRequired.value) return true
  if (isAuthenticated.value) return true

  return { name: 'login' }
})

export default router
