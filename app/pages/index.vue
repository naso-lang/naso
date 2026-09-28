<template>
  <div class="min-h-screen bg-canvas text-text-primary font-sans">
    <!-- Telemetry Bar -->
    <header class="telemetry-bar" aria-label="System telemetry">
      <div class="flex items-center gap-6">
        <!-- WebGPU Status -->
        <div class="flex items-center gap-2">
          <span class="status-dot active" :class="{ 
            active: telemetry.webgpuStatus === 'connected',
            warning: telemetry.webgpuStatus === 'warning',
            error: telemetry.webgpuStatus === 'error'
          }" aria-label="WebGPU status" />
          <span class="text-xs font-medium text-text-primary">WebGPU</span>
          <span class="telemetry-value font-mono-tabular" :class="telemetry.webgpuStatus === 'connected' ? 'text-accent' : 'text-error'">
            {{ telemetry.webgpuStatus === 'connected' ? 'ACTIVE' : telemetry.webgpuStatus.toUpperCase() }}
          </span>
        </div>

        <!-- VRAM -->
        <div class="telemetry-item">
          <span class="text-text-dimmed">VRAM</span>
          <span class="telemetry-value">{{ formatBytes(telemetry.vramAllocated) }}</span>
        </div>

        <!-- KV Cache -->
        <div class="telemetry-item">
          <span class="text-text-dimmed">KV-CACHE</span>
          <span class="telemetry-value">{{ formatBytes(telemetry.kvCacheMemory) }}</span>
        </div>

        <!-- Throughput -->
        <div class="telemetry-item">
          <span class="text-text-dimmed">TOK/S</span>
          <span class="telemetry-value font-mono-tabular">{{ telemetry.tokensPerSecond.toFixed(1) }}</span>
        </div>

        <!-- Active Workgroups -->
        <div class="telemetry-item">
          <span class="text-text-dimmed">WORKGROUPS</span>
          <span class="telemetry-value font-mono-tabular">{{ telemetry.activeWorkgroups }}</span>
        </div>
      </div>

      <div class="flex items-center gap-4">
        <!-- Model indicator -->
        <span class="text-xs text-text-dimmed">Naso-INT8</span>
        <!-- Connection status -->
        <span class="text-xs text-text-dimmed" :class="telemetry.webgpuStatus === 'connected' ? 'text-accent' : 'text-error'">
          {{ telemetry.webgpuStatus === 'connected' ? '● CONNECTED' : '● DISCONNECTED' }}
        </span>
      </div>
    </header>

    <!-- Main Content Area -->
    <main class="pt-telemetry pb-24 px-4 max-w-4xl mx-auto">
      <div class="space-y-4" ref="messagesContainer">
        <!-- Welcome message -->
        <div v-if="messages.length === 0" class="message-block message-assistant">
          <p class="text-text-muted text-sm">
            NasoChat ready. WebGPU compute shaders loaded. Press <kbd class="px-1.5 py-0.5 bg-surface border border-border-subtle rounded text-xs font-mono">Cmd+K</kbd> to open command palette.
          </p>
        </div>

        <!-- Messages -->
        <div 
          v-for="(msg, idx) in messages" 
          :key="idx"
          :class="['message-block', msg.role === 'user' ? 'message-user' : 'message-assistant']"
        >
          <div class="flex items-start gap-3">
            <span class="text-xs text-text-dimmed font-mono shrink-0 mt-1">
              {{ msg.role === 'user' ? '>' : '▸' }}
            </span>
            <div class="flex-1 min-w-0">
              <p v-if="msg.streaming" class="whitespace-pre-wrap text-sm leading-relaxed">
                {{ msg.content }}<span class="animate-pulse-subtle text-accent">█</span>
              </p>
              <p v-else class="whitespace-pre-wrap text-sm leading-relaxed">{{ msg.content }}</p>
            </div>
          </div>
        </div>
      </div>
    </main>

    <!-- Command Palette -->
    <div 
      v-if="showPalette" 
      class="command-palette" 
      role="dialog" 
      aria-modal="true" 
      aria-label="Command palette"
      @keydown.escape="closePalette"
    >
      <div class="px-4 py-3 border-b border-border-subtle flex items-center gap-3">
        <span class="text-xs text-text-dimmed font-mono">CMD</span>
        <input
          ref="paletteInput"
          v-model="paletteInputValue"
          type="text"
          class="command-palette-input"
          placeholder="Enter prompt or /command..."
          @keydown.enter="handlePaletteSubmit"
          @keydown.up="navigateHistory(-1)"
          @keydown.down="navigateHistory(1)"
          autocomplete="off"
          spellcheck="false"
        />
        <span v-if="paletteInputValue" class="text-xs text-text-dimmed font-mono-tabular">
          {{ paletteInputValue.length }} chars
        </span>
      </div>
      
      <!-- Command suggestions -->
      <div v-if="filteredCommands.length > 0 && paletteInputValue.startsWith('/')" class="border-t border-border-subtle max-h-48 overflow-y-auto">
        <div 
          v-for="(cmd, idx) in filteredCommands" 
          :key="cmd.name"
          :class="['px-4 py-2 text-sm hover:bg-surface/50 cursor-pointer flex items-center gap-3', idx === selectedCommandIndex ? 'bg-surface/50' : '']"
          @click="executeCommand(cmd)"
          @mouseenter="selectedCommandIndex = idx"
        >
          <span class="text-accent font-mono">{{ cmd.name }}</span>
          <span class="text-text-muted">{{ cmd.description }}</span>
        </div>
      </div>

      <!-- History -->
      <div v-else-if="commandHistory.length > 0 && !paletteInputValue" class="border-t border-border-subtle max-h-48 overflow-y-auto">
        <div class="px-3 py-2 text-xs text-text-dimmed uppercase tracking-wider">Recent</div>
        <div 
          v-for="(hist, idx) in [...commandHistory].reverse().slice(0, 8)" 
          :key="idx"
          class="px-4 py-1.5 text-sm hover:bg-surface/50 cursor-pointer border-b border-border-subtle/50 last:border-0"
          @click="paletteInputValue = hist"
        >
          {{ hist.length > 80 ? hist.slice(0, 80) + '…' : hist }}
        </div>
      </div>
    </div>

    <!-- Keyboard shortcut hint -->
    <div class="fixed bottom-4 right-4 text-xs text-text-dimmed/50 hidden md:block">
      <kbd class="px-2 py-1 bg-surface border border-border-subtle rounded font-mono">⌘K</kbd> Command palette
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, computed, onMounted, onUnmounted, nextTick, watch } from 'vue'
import { useNasoEngine } from '~/composables/useNasoEngine'

interface Message {
  role: 'user' | 'assistant'
  content: string
  streaming?: boolean
}

interface Command {
  name: string
  description: string
  handler: () => void
}

// Engine
const { 
  initializeWebGPU, 
  initializeWorker, 
  telemetry, 
  onStreamChunk, 
  sendStreamChunk,
  cleanup 
} = useNasoEngine()

// State
const messages = ref<Message[]>([])
const showPalette = ref(false)
const paletteInputValue = ref('')
const paletteInput = ref<HTMLInputElement | null>(null)
const messagesContainer = ref<HTMLElement | null>(null)
const commandHistory: string[] = []
const historyIndex = ref(-1)
const selectedCommandIndex = ref(-1)

// Commands
const commands: Command[] = [
  { name: '/clear', description: 'Clear conversation history', handler: () => { messages.value = [] } },
  { name: '/quantize', description: 'Run INT8 quantization demo', handler: runQuantizeDemo },
  { name: '/dequantize', description: 'Run INT8 dequantization demo', handler: runDequantizeDemo },
  { name: '/status', description: 'Show system status', handler: showStatus },
  { name: '/help', description: 'List available commands', handler: showHelp },
]

const filteredCommands = computed(() => {
  const query = paletteInputValue.value.slice(1).toLowerCase()
  return commands.filter(c => c.name.slice(1).includes(query))
})

// Initialize
onMounted(async () => {
  await initializeWebGPU()
  initializeWorker()
  
  // Subscribe to stream chunks
  const unsubscribe = onStreamChunk((chunk) => {
    handleStreamChunk(chunk)
  })

  // Keyboard shortcut: Cmd/Ctrl + K
  const handleKeydown = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
      e.preventDefault()
      togglePalette()
    }
  }
  document.addEventListener('keydown', handleKeydown)

  // Cleanup on unmount
  onUnmounted(() => {
    document.removeEventListener('keydown', handleKeydown)
    unsubscribe()
    cleanup()
  })
})

// Palette handlers
function togglePalette(): void {
  showPalette.value = !showPalette.value
  if (showPalette.value) {
    paletteInputValue.value = ''
    selectedCommandIndex.value = -1
    historyIndex.value = -1
    nextTick(() => {
      paletteInput.value?.focus()
    })
  }
}

function closePalette(): void {
  showPalette.value = false
  paletteInputValue.value = ''
  selectedCommandIndex.value = -1
}

async function handlePaletteSubmit(): void {
  const input = paletteInputValue.value.trim()
  if (!input) return

  if (input.startsWith('/')) {
    const cmd = commands.find(c => c.name === input.split(' ')[0])
    if (cmd) {
      commandHistory.push(input)
      if (commandHistory.length > 50) commandHistory.shift()
      cmd.handler()
      closePalette()
      return
    }
  }

  // Send as user message
  await sendUserMessage(input)
  commandHistory.push(input)
  if (commandHistory.length > 50) commandHistory.shift()
  closePalette()
}

function navigateHistory(direction: number): void {
  const history = [...commandHistory].reverse()
  if (history.length === 0) return
  
  historyIndex.value = Math.max(0, Math.min(history.length - 1, historyIndex.value + direction))
  paletteInputValue.value = history[historyIndex.value] || ''
}

function executeCommand(cmd: Command): void {
  commandHistory.push(cmd.name)
  if (commandHistory.length > 50) commandHistory.shift()
  cmd.handler()
  closePalette()
}

// Message handling
async function sendUserMessage(content: string): Promise<void> {
  // Add user message
  messages.value.push({ role: 'user', content })
  scrollToBottom()

  // Add streaming assistant message
  const assistantMsg: Message = { role: 'assistant', content: '', streaming: true }
  messages.value.push(assistantMsg)
  scrollToBottom()

  // Simulate streaming response (replace with actual LLM streaming)
  const response = await generateResponse(content)
  assistantMsg.streaming = false
  assistantMsg.content = response
  scrollToBottom()
}

function handleStreamChunk(chunk: string): void {
  const lastMsg = messages.value[messages.value.length - 1]
  if (lastMsg && lastMsg.role === 'assistant' && lastMsg.streaming) {
    lastMsg.content += chunk
    scrollToBottom()
  }
}

async function generateResponse(prompt: string): Promise<string> {
  // This would connect to actual LLM streaming
  // For now, simulate with a delay
  const responses = [
    'Processing through Naso INT8 symmetric quantization kernel...',
    'WebGPU compute shader dispatched with 256 workgroup size.',
    'Quantization complete. Output tensor ready in storage buffer.',
    'Dequantization verification passed. Linear invariants preserved.',
  ]
  
  let fullResponse = ''
  for (const part of responses) {
    fullResponse += part + '\n'
    await new Promise(r => setTimeout(r, 100))
  }
  return fullResponse.trim()
}

function scrollToBottom(): void {
  nextTick(() => {
    messagesContainer.value?.scrollTo({
      top: messagesContainer.value.scrollHeight,
      behavior: 'smooth',
    })
  })
}

// Demo commands
async function runQuantizeDemo(): Promise<void> {
  const { quantize } = useNasoEngine()
  try {
    const input = new Float32Array(1024)
    for (let i = 0; i < input.length; i++) {
      input[i] = (Math.random() - 0.5) * 10
    }
    const scale = 0.1
    const start = performance.now()
    const output = await quantize(input, scale)
    const elapsed = performance.now() - start
    
    messages.value.push({
      role: 'assistant',
      content: `Quantization complete in ${elapsed.toFixed(2)}ms\nInput: Float32[${input.length}]\nOutput: Int32[${output.length}]\nScale: ${scale}\nSample: [${Array.from(output.slice(0, 8)).join(', ')}...]`
    })
    scrollToBottom()
    
    // Update telemetry
    telemetry.tokensPerSecond = input.length / (elapsed / 1000)
  } catch (error) {
    messages.value.push({
      role: 'assistant',
      content: `Quantization failed: ${error instanceof Error ? error.message : String(error)}`
    })
  }
}

async function runDequantizeDemo(): Promise<void> {
  const { dequantize } = useNasoEngine()
  try {
    const input = new Int32Array(1024)
    for (let i = 0; i < input.length; i++) {
      input[i] = Math.floor((Math.random() - 0.5) * 200)
    }
    const scale = 0.1
    const start = performance.now()
    const output = await dequantize(input, scale)
    const elapsed = performance.now() - start
    
    messages.value.push({
      role: 'assistant',
      content: `Dequantization complete in ${elapsed.toFixed(2)}ms\nInput: Int32[${input.length}]\nOutput: Float32[${output.length}]\nScale: ${scale}\nSample: [${Array.from(output.slice(0, 8)).map(v => v.toFixed(4)).join(', ')}...]`
    })
    scrollToBottom()
    
    telemetry.tokensPerSecond = input.length / (elapsed / 1000)
  } catch (error) {
    messages.value.push({
      role: 'assistant',
      content: `Dequantization failed: ${error instanceof Error ? error.message : String(error)}`
    })
  }
}

function showStatus(): void {
  messages.value.push({
    role: 'assistant',
    content: `System Status:\nWebGPU: ${telemetry.webgpuStatus}\nVRAM: ${formatBytes(telemetry.vramAllocated)}\nKV-Cache: ${formatBytes(telemetry.kvCacheMemory)}\nThroughput: ${telemetry.tokensPerSecond.toFixed(1)} tok/s\nWorkgroups: ${telemetry.activeWorkgroups}`
  })
  scrollToBottom()
}

function showHelp(): void {
  messages.value.push({
    role: 'assistant',
    content: 'Available commands:\n' + commands.map(c => `  ${c.name} — ${c.description}`).join('\n')
  })
  scrollToBottom()
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

// Watch telemetry for VRAM warnings
watch(() => telemetry.vramAllocated, (newVal) => {
  if (newVal > 100 * 1024 * 1024) { // 100MB
    telemetry.webgpuStatus = 'warning'
  }
})
</script>

<style scoped>
/* Ensure smooth scrolling */
html {
  scroll-behavior: smooth;
}

/* Focus styles for accessibility */
input:focus-visible,
button:focus-visible {
  outline: 2px solid #10B981;
  outline-offset: 2px;
}

/* Custom scrollbar for command palette */
.command-palette::-webkit-scrollbar {
  width: 6px;
}

.command-palette::-webkit-scrollbar-track {
  background: #050505;
}

.command-palette::-webkit-scrollbar-thumb {
  background: #27272A;
  border-radius: 3px;
}

/* Message animation */
.message-block {
  animation: fadeIn 0.15s ease-out;
}

@keyframes fadeIn {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>