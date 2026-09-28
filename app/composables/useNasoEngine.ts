import { ref, shallowRef, onUnmounted, computed } from 'vue'
import type { GPUDevice, GPUAdapter, GPUComputePipeline, GPUBindGroup, GPUBuffer } from 'webgpu-types'

interface WebGPUState {
  adapter: GPUAdapter | null
  device: GPUDevice | null
  isInitialized: boolean
  error: string | null
}

interface KernelPipeline {
  pipeline: GPUComputePipeline
  bindGroupLayout: GPUBindGroupLayout
}

interface StreamBuffer {
  buffer: SharedArrayBuffer
  writeIndex: number
  readIndex: number
  view: Uint8Array
}

interface TelemetryData {
  vramAllocated: number
  kvCacheMemory: number
  tokensPerSecond: number
  webgpuStatus: 'connected' | 'disconnected' | 'error'
  activeWorkgroups: number
}

const QUANTIZE_WGSL = `
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<i32>;
@group(0) @binding(2) var<uniform> scale: f32;
@group(0) @binding(3) var<uniform> N: u32;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if (idx >= N) { return; }
    
    let val = input[idx];
    let scaled = val / scale;
    let rounded = round(scaled);
    let quantized = i32(clamp(rounded, -128.0, 127.0));
    output[idx] = quantized;
}
`

const DEQUANTIZE_WGSL = `
@group(0) @binding(0) var<storage, read> input: array<i32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> scale: f32;
@group(0) @binding(3) var<uniform> N: u32;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if (idx >= N) { return; }
    
    let val = input[idx];
    let dequantized = f32(val) * scale;
    output[idx] = dequantized;
}
`

export function useNasoEngine() {
  // WebGPU state
  const webgpuState = reactive<WebGPUState>({
    adapter: null,
    device: null,
    isInitialized: false,
    error: null,
  })

  // Pipelines cache
  const pipelines = shallowRef<Map<string, KernelPipeline>>(new Map())
  
  // GPU buffers cache
  const gpuBuffers = shallowRef<Map<string, GPUBuffer>>(new Map())

  // Streaming ring buffer
  const streamBuffer = shallowRef<StreamBuffer | null>(null)
  
  // Telemetry
  const telemetry = reactive<TelemetryData>({
    vramAllocated: 0,
    kvCacheMemory: 0,
    tokensPerSecond: 0,
    webgpuStatus: 'disconnected',
    activeWorkgroups: 0,
  })

  // Web Worker
  const worker = shallowRef<Worker | null>(null)

  // Initialize WebGPU device
  async function initializeWebGPU(): Promise<GPUDevice | null> {
    if (webgpuState.isInitialized && webgpuState.device) {
      return webgpuState.device
    }

    if (!navigator.gpu) {
      webgpuState.error = 'WebGPU not supported in this browser'
      webgpuState.webgpuStatus = 'error'
      return null
    }

    try {
      const adapter = await navigator.gpu.requestAdapter({
        powerPreference: 'high-performance',
      })

      if (!adapter) {
        webgpuState.error = 'No WebGPU adapter available'
        webgpuState.webgpuStatus = 'error'
        return null
      }

      const device = await adapter.requestDevice({
        requiredFeatures: ['timestamp-query', 'subgroups'],
        requiredLimits: {
          maxComputeWorkgroupsPerDimension: 65535,
          maxComputeInvocationsPerWorkgroup: 256,
          maxStorageBufferBindingSize: 256 * 1024 * 1024,
        },
        label: 'NasoChat WebGPU Device',
      })

      // Handle device loss
      device.lost.then((info) => {
        console.warn('[NasoEngine] WebGPU device lost:', info.reason)
        webgpuState.isInitialized = false
        webgpuState.device = null
        webgpuState.webgpuStatus = 'disconnected'
        telemetry.webgpuStatus = 'disconnected'
      })

      webgpuState.adapter = adapter
      webgpuState.device = device
      webgpuState.isInitialized = true
      webgpuState.webgpuStatus = 'connected'
      telemetry.webgpuStatus = 'connected'
      webgpuState.error = null

      console.log('[NasoEngine] WebGPU initialized successfully')
      return device
    } catch (error) {
      webgpuState.error = error instanceof Error ? error.message : 'WebGPU initialization failed'
      webgpuState.webgpuStatus = 'error'
      telemetry.webgpuStatus = 'error'
      console.error('[NasoEngine] WebGPU init error:', error)
      return null
    }
  }

  // Get or create compute pipeline
  async function getPipeline(name: 'quantize' | 'dequantize'): Promise<KernelPipeline> {
    const device = webgpuState.device
    if (!device) {
      throw new Error('WebGPU device not initialized')
    }

    if (pipelines.value.has(name)) {
      return pipelines.value.get(name)!
    }

    const wgslCode = name === 'quantize' ? QUANTIZE_WGSL : DEQUANTIZE_WGSL
    const shaderModule = device.createShaderModule({
      code: wgslCode,
      label: `Naso ${name} shader`,
    })

    const pipeline = device.createComputePipeline({
      layout: 'auto',
      compute: {
        module: shaderModule,
        entryPoint: 'main',
      },
      label: `Naso ${name} pipeline`,
    })

    const kernelPipeline: KernelPipeline = {
      pipeline,
      bindGroupLayout: pipeline.getBindGroupLayout(0),
    }

    pipelines.value.set(name, kernelPipeline)
    return kernelPipeline
  }

  // Create GPU buffers for kernel execution
  function createStorageBuffer(data: Float32Array | Int32Array, usage: GPUBufferUsageFlags): GPUBuffer {
    const device = webgpuState.device!
    const buffer = device.createBuffer({
      size: data.byteLength,
      usage: usage | GPUBufferUsage.COPY_DST,
      mappedAtCreation: true,
    })
    
    if (data instanceof Float32Array) {
      new Float32Array(buffer.getMappedRange()).set(data)
    } else {
      new Int32Array(buffer.getMappedRange()).set(data)
    }
    buffer.unmap()
    return buffer
  }

  function createUniformBuffer(scale: number, N: number): GPUBuffer {
    const device = webgpuState.device!
    const buffer = device.createBuffer({
      size: 8,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
      mappedAtCreation: true,
    })
    const view = new DataView(buffer.getMappedRange())
    view.setFloat32(0, scale, true)
    view.setUint32(4, N, true)
    buffer.unmap()
    return buffer
  }

  function createReadbackBuffer(byteLength: number): GPUBuffer {
    const device = webgpuState.device!
    return device.createBuffer({
      size: byteLength,
      usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
    })
  }

  // Execute quantization kernel
  async function quantize(
    input: Float32Array,
    scale: number
  ): Promise<Int32Array> {
    const device = webgpuState.device || (await initializeWebGPU())
    if (!device) {
      throw new Error('WebGPU unavailable')
    }

    const N = input.length
    const output = new Int32Array(N)

    const { pipeline, bindGroupLayout } = await getPipeline('quantize')

    // Create buffers
    const inputBuffer = createStorageBuffer(input, GPUBufferUsage.STORAGE)
    const outputBuffer = device.createBuffer({
      size: output.byteLength,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
    })
    const uniformBuffer = createUniformBuffer(scale, N)

    // Bind group
    const bindGroup = device.createBindGroup({
      layout: bindGroupLayout,
      entries: [
        { binding: 0, resource: { buffer: inputBuffer } },
        { binding: 1, resource: { buffer: outputBuffer } },
        { binding: 2, resource: { buffer: uniformBuffer } },
        { binding: 3, resource: { buffer: uniformBuffer } },
      ],
    })

    // Execute
    const commandEncoder = device.createCommandEncoder()
    const passEncoder = commandEncoder.beginComputePass()
    passEncoder.setPipeline(pipeline)
    passEncoder.setBindGroup(0, bindGroup)
    const workgroupCount = Math.ceil(N / 256)
    passEncoder.dispatchWorkgroups(workgroupCount)
    telemetry.activeWorkgroups = workgroupCount
    passEncoder.end()

    // Readback
    const readbackBuffer = createReadbackBuffer(output.byteLength)
    commandEncoder.copyBufferToBuffer(outputBuffer, 0, readbackBuffer, 0, output.byteLength)

    device.queue.submit([commandEncoder.finish()])

    await readbackBuffer.mapAsync(GPUMapMode.READ)
    const result = new Int32Array(readbackBuffer.getMappedRange())
    output.set(result)
    readbackBuffer.unmap()

    // Update telemetry
    telemetry.vramAllocated += inputBuffer.size + outputBuffer.size + uniformBuffer.size + readbackBuffer.size

    // Cleanup
    inputBuffer.destroy()
    outputBuffer.destroy()
    uniformBuffer.destroy()
    readbackBuffer.destroy()

    return output
  }

  // Execute dequantization kernel
  async function dequantize(
    input: Int32Array,
    scale: number
  ): Promise<Float32Array> {
    const device = webgpuState.device || (await initializeWebGPU())
    if (!device) {
      throw new Error('WebGPU unavailable')
    }

    const N = input.length
    const output = new Float32Array(N)

    const { pipeline, bindGroupLayout } = await getPipeline('dequantize')

    const inputBuffer = device.createBuffer({
      size: input.byteLength,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
      mappedAtCreation: true,
    })
    new Int32Array(inputBuffer.getMappedRange()).set(input)
    inputBuffer.unmap()

    const outputBuffer = device.createBuffer({
      size: output.byteLength,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
    })
    const uniformBuffer = createUniformBuffer(scale, N)

    const bindGroup = device.createBindGroup({
      layout: bindGroupLayout,
      entries: [
        { binding: 0, resource: { buffer: inputBuffer } },
        { binding: 1, resource: { buffer: outputBuffer } },
        { binding: 2, resource: { buffer: uniformBuffer } },
        { binding: 3, resource: { buffer: uniformBuffer } },
      ],
    })

    const commandEncoder = device.createCommandEncoder()
    const passEncoder = commandEncoder.beginComputePass()
    passEncoder.setPipeline(pipeline)
    passEncoder.setBindGroup(0, bindGroup)
    const workgroupCount = Math.ceil(N / 256)
    passEncoder.dispatchWorkgroups(workgroupCount)
    telemetry.activeWorkgroups = workgroupCount
    passEncoder.end()

    const readbackBuffer = createReadbackBuffer(output.byteLength)
    commandEncoder.copyBufferToBuffer(outputBuffer, 0, readbackBuffer, 0, output.byteLength)

    device.queue.submit([commandEncoder.finish()])

    await readbackBuffer.mapAsync(GPUMapMode.READ)
    const result = new Float32Array(readbackBuffer.getMappedRange())
    output.set(result)
    readbackBuffer.unmap()

    telemetry.vramAllocated += inputBuffer.size + outputBuffer.size + uniformBuffer.size + readbackBuffer.size

    inputBuffer.destroy()
    outputBuffer.destroy()
    uniformBuffer.destroy()
    readbackBuffer.destroy()

    return output
  }

  // Initialize Web Worker
  function initializeWorker(): Worker {
    if (worker.value) {
      return worker.value
    }

    const w = new Worker(new URL('../workers/naso-runner.worker.ts', import.meta.url), {
      type: 'module',
      name: 'naso-runner',
    })

    w.onmessage = (event) => {
      const { type, payload } = event.data
      
      switch (type) {
        case 'ready':
          if (payload?.ringBuffer) {
            streamBuffer.value = {
              buffer: payload.ringBuffer,
              writeIndex: 0,
              readIndex: 0,
              view: new Uint8Array(payload.ringBuffer),
            }
            console.log('[NasoEngine] Worker ready, streaming buffer initialized')
          }
          break
          
        case 'stream-chunk':
          // Handle streaming token
          if (payload?.chunk) {
            handleStreamChunk(payload.chunk)
          }
          break
          
        case 'result':
          // Handle kernel result
          break
          
        case 'error':
          console.error('[NasoEngine] Worker error:', payload?.error)
          break
      }
    }

    w.onerror = (error) => {
      console.error('[NasoEngine] Worker error:', error)
    }

    worker.value = w
    
    // Initialize worker with WebGPU
    w.postMessage({ type: 'init' })

    return w
  }

  // Stream token handling
  const streamCallbacks = ref<Set<(chunk: string) => void>>(new Set())

  function onStreamChunk(callback: (chunk: string) => void): () => void {
    streamCallbacks.value.add(callback)
    return () => streamCallbacks.value.delete(callback)
  }

  function handleStreamChunk(chunk: string): void {
    for (const callback of streamCallbacks.value) {
      callback(chunk)
    }
  }

  // Send stream chunk to worker
  function sendStreamChunk(chunk: string): void {
    if (worker.value) {
      worker.value.postMessage({ type: 'stream', payload: { chunk } })
    }
  }

  // Initialize streaming ring buffer
  function initializeStreamBuffer(size = 64 * 1024): SharedArrayBuffer {
    const buffer = new SharedArrayBuffer(size)
    streamBuffer.value = {
      buffer,
      writeIndex: 0,
      readIndex: 0,
      view: new Uint8Array(buffer),
    }
    return buffer
  }

  // Read from stream buffer
  function readStreamBuffer(): string | null {
    if (!streamBuffer.value) return null
    
    const { view, readIndex, writeIndex } = streamBuffer.value
    if (readIndex === writeIndex) return null

    let end = writeIndex
    if (writeIndex < readIndex) {
      end = view.length
    }
    
    const chunk = new TextDecoder().decode(view.slice(readIndex, end))
    streamBuffer.value.readIndex = end % view.length
    
    return chunk
  }

  // Cleanup
  function cleanup(): void {
    // Destroy GPU resources
    for (const [, buffer] of gpuBuffers.value) {
      buffer.destroy()
    }
    gpuBuffers.value.clear()
    pipelines.value.clear()

    // Terminate worker
    if (worker.value) {
      worker.value.postMessage({ type: 'terminate' })
      worker.value.terminate()
      worker.value = null
    }

    // Reset state
    webgpuState.isInitialized = false
    webgpuState.device = null
    webgpuState.adapter = null
    telemetry.webgpuStatus = 'disconnected'
    streamBuffer.value = null
  }

  onUnmounted(() => {
    cleanup()
  })

  // Computed helpers
  const isWebGPUAvailable = computed(() => webgpuState.isInitialized)
  const hasError = computed(() => !!webgpuState.error)

  return {
    // State
    webgpuState: readonly(webgpuState),
    telemetry: readonly(telemetry),
    
    // Initialization
    initializeWebGPU,
    initializeWorker,
    
    // Kernels
    quantize,
    dequantize,
    
    // Streaming
    onStreamChunk,
    sendStreamChunk,
    initializeStreamBuffer,
    readStreamBuffer,
    
    // Cleanup
    cleanup,
    
    // Computed
    isWebGPUAvailable,
    hasError,
  }
}

// Re-export types
export type { WebGPUState, TelemetryData, StreamBuffer }