import './assets/main.css'

import { createApp } from 'vue'
import ui from '@nuxt/ui/vue-plugin'
import App from './App.vue'
import router from './router'
import { client } from './client/client.gen'

// Redirect to login on 401 responses from the API
client.interceptors.response.use((response) => {
  if (response.status === 401) {
    router.push({ name: 'login' })
  }
  return response
})

const app = createApp(App)

app.use(router)
app.use(ui)

app.mount('#app')
