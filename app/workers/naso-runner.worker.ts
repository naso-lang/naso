/// <reference types="vite/client" />

// Web Worker for Naso kernel execution
// Offloads WASM/kernel computation from main UI thread

interface NasoWorkerMessage {
  type: 'init' | 'execute' | 'stream' | 'terminate';
  payload?: {
    kernelName?: string;
    wasmModule?: WebAssembly.Module;
    inputBuffers?: Float32Array[];
    outputBuffers?: Float32Array[];
    scale?: number;
    workgroupCount?: number;
    workgroupSize?: number;
  };
}

interface NasoWorkerResponse {
  type: 'ready' | 'result' | 'progress' | 'error' | 'stream-chunk';
  payload?: {
    outputBuffers?: Float32Array[];
    progress?: number;
    chunk?: string;
    error?: string;
  };
}

let nasoWasmModule: WebAssembly.Module | null = null;
let nasoWasmInstance: WebAssembly.Instance | null = null;
let wasmMemory: WebAssembly.Memory | null = null;

// WebGPU context for compute shaders
let gpuDevice: GPUDevice | null = null;
let computePipelines: Map<string, GPUComputePipeline> = new Map();
let gpuBuffers: Map<string, GPUBuffer> = new Map();

// SharedArrayBuffer for streaming
let streamRingBuffer: SharedArrayBuffer | null = null;
let streamWriteIndex = 0;
let streamReadIndex = 0;

async function initializeWebGPU(): Promise<GPUDevice | null> {
  if (!navigator.gpu) {
    console.warn('[NasoWorker] WebGPU not available');
    return null;
  }

  try {
    const adapter = await navigator.gpu.requestAdapter({
      powerPreference: 'high-performance',
    });

    if (!adapter) {
      console.warn('[NasoWorker] No WebGPU adapter found');
      return null;
    }

    const device = await adapter.requestDevice({
      requiredFeatures: ['timestamp-query', 'subgroups'],
      requiredLimits: {
        maxComputeWorkgroupsPerDimension: 65535,
        maxComputeInvocationsPerWorkgroup: 256,
        maxStorageBufferBindingSize: 256 * 1024 * 1024,
      },
    });

    console.log('[NasoWorker] WebGPU device initialized');
    return device;
  } catch (error) {
    console.error('[NasoWorker] WebGPU initialization failed:', error);
    return null;
  }
}

async function loadWGSLShader(device: GPUDevice, shaderCode: string): Promise<GPUShaderModule> {
  return device.createShaderModule({
    code: shaderCode,
    label: 'Naso Quant Kernel',
  });
}

function createComputePipeline(
  device: GPUDevice,
  shaderModule: GPUShaderModule,
  entryPoint: string
): GPUComputePipeline {
  return device.createComputePipeline({
    layout: 'auto',
    compute: {
      module: shaderModule,
      entryPoint,
    },
    label: `Naso Compute Pipeline: ${entryPoint}`,
  });
}

async function executeQuantizeKernel(
  inputBuffer: Float32Array,
  outputBuffer: Float32Array,
  scale: number,
  N: number
): Promise<void> {
  if (!gpuDevice) {
    gpuDevice = await initializeWebGPU();
    if (!gpuDevice) {
      throw new Error('WebGPU unavailable');
    }
  }

  // Create or reuse pipeline
  let pipeline = computePipelines.get('quantize_int8_symmetric');
  if (!pipeline) {
    // WGSL shader for INT8 symmetric quantization
    const wgslCode = `
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
`;

    const shaderModule = await loadWGSLShader(gpuDevice, wgslCode);
    pipeline = createComputePipeline(gpuDevice, shaderModule, 'main');
    computePipelines.set('quantize_int8_symmetric', pipeline);
  }

  // Create buffers
  const inputGpuBuffer = gpuDevice.createBuffer({
    size: inputBuffer.byteLength,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    mappedAtCreation: true,
  });
  new Float32Array(inputGpuBuffer.getMappedRange()).set(inputBuffer);
  inputGpuBuffer.unmap();

  const outputGpuBuffer = gpuDevice.createBuffer({
    size: outputBuffer.byteLength,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });

  const uniformBuffer = gpuDevice.createBuffer({
    size: 8, // f32 + u32 = 8 bytes
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    mappedAtCreation: true,
  });
  const uniformView = new DataView(uniformBuffer.getMappedRange());
  uniformView.setFloat32(0, scale, true);
  uniformView.setUint32(4, N, true);
  uniformBuffer.unmap();

  // Bind group
  const bindGroup = gpuDevice.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: inputGpuBuffer } },
      { binding: 1, resource: { buffer: outputGpuBuffer } },
      { binding: 2, resource: { buffer: uniformBuffer } },
      { binding: 3, resource: { buffer: uniformBuffer } },
    ],
  });

  // Execute
  const commandEncoder = gpuDevice.createCommandEncoder();
  const passEncoder = commandEncoder.beginComputePass();
  passEncoder.setPipeline(pipeline);
  passEncoder.setBindGroup(0, bindGroup);
  const workgroupCount = Math.ceil(N / 256);
  passEncoder.dispatchWorkgroups(workgroupCount);
  passEncoder.end();

  // Read back
  const readbackBuffer = gpuDevice.createBuffer({
    size: outputBuffer.byteLength,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  commandEncoder.copyBufferToBuffer(outputGpuBuffer, 0, readbackBuffer, 0, outputBuffer.byteLength);

  gpuDevice.queue.submit([commandEncoder.finish()]);

  await readbackBuffer.mapAsync(GPUMapMode.READ);
  const result = new Int32Array(readbackBuffer.getMappedRange());
  const outputView = new Int32Array(outputBuffer.buffer);
  outputView.set(result);
  readbackBuffer.unmap();

  // Cleanup
  inputGpuBuffer.destroy();
  outputGpuBuffer.destroy();
  uniformBuffer.destroy();
  readbackBuffer.destroy();
}

async function executeDequantizeKernel(
  inputBuffer: Float32Array,
  outputBuffer: Float32Array,
  scale: number,
  N: number
): Promise<void> {
  if (!gpuDevice) {
    gpuDevice = await initializeWebGPU();
    if (!gpuDevice) {
      throw new Error('WebGPU unavailable');
    }
  }

  let pipeline = computePipelines.get('dequantize_int8_symmetric');
  if (!pipeline) {
    const wgslCode = `
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
`;

    const shaderModule = await loadWGSLShader(gpuDevice, wgslCode);
    pipeline = createComputePipeline(gpuDevice, shaderModule, 'main');
    computePipelines.set('dequantize_int8_symmetric', pipeline);
  }

  const inputGpuBuffer = gpuDevice.createBuffer({
    size: inputBuffer.byteLength,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    mappedAtCreation: true,
  });
  new Int32Array(inputGpuBuffer.getMappedRange()).set(new Int32Array(inputBuffer.buffer));
  inputGpuBuffer.unmap();

  const outputGpuBuffer = gpuDevice.createBuffer({
    size: outputBuffer.byteLength,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });

  const uniformBuffer = gpuDevice.createBuffer({
    size: 8,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    mappedAtCreation: true,
  });
  const uniformView = new DataView(uniformBuffer.getMappedRange());
  uniformView.setFloat32(0, scale, true);
  uniformView.setUint32(4, N, true);
  uniformBuffer.unmap();

  const bindGroup = gpuDevice.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: inputGpuBuffer } },
      { binding: 1, resource: { buffer: outputGpuBuffer } },
      { binding: 2, resource: { buffer: uniformBuffer } },
      { binding: 3, resource: { buffer: uniformBuffer } },
    ],
  });

  const commandEncoder = gpuDevice.createCommandEncoder();
  const passEncoder = commandEncoder.beginComputePass();
  passEncoder.setPipeline(pipeline);
  passEncoder.setBindGroup(0, bindGroup);
  const workgroupCount = Math.ceil(N / 256);
  passEncoder.dispatchWorkgroups(workgroupCount);
  passEncoder.end();

  const readbackBuffer = gpuDevice.createBuffer({
    size: outputBuffer.byteLength,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  commandEncoder.copyBufferToBuffer(outputGpuBuffer, 0, readbackBuffer, 0, outputBuffer.byteLength);

  gpuDevice.queue.submit([commandEncoder.finish()]);

  await readbackBuffer.mapAsync(GPUMapMode.READ);
  const result = new Float32Array(readbackBuffer.getMappedRange());
  outputBuffer.set(result);
  readbackBuffer.unmap();

  inputGpuBuffer.destroy();
  outputGpuBuffer.destroy();
  uniformBuffer.destroy();
  readbackBuffer.destroy();
}

// Message handler
self.onmessage = async (event: MessageEvent<NasoWorkerMessage>) => {
  const { type, payload } = event.data;

  switch (type) {
    case 'init': {
      if (payload?.wasmModule) {
        nasoWasmModule = payload.wasmModule;
        nasoWasmInstance = new WebAssembly.Instance(nasoWasmModule);
        wasmMemory = nasoWasmInstance.exports.memory as WebAssembly.Memory;
      }
      
      // Initialize WebGPU
      gpuDevice = await initializeWebGPU();
      
      // Initialize streaming ring buffer (64KB)
      streamRingBuffer = new SharedArrayBuffer(64 * 1024);
      streamWriteIndex = 0;
      streamReadIndex = 0;

      const response: NasoWorkerResponse = {
        type: 'ready',
        payload: { ringBuffer: streamRingBuffer },
      };
      self.postMessage(response, [streamRingBuffer!]);
      break;
    }

    case 'execute': {
      try {
        const { kernelName, inputBuffers, outputBuffers, scale, workgroupCount } = payload!;
        
        if (kernelName === 'quantize_int8_symmetric') {
          await executeQuantizeKernel(
            inputBuffers![0],
            outputBuffers![0],
            scale!,
            inputBuffers![0].length
          );
        } else if (kernelName === 'dequantize_int8_symmetric') {
          await executeDequantizeKernel(
            inputBuffers![0],
            outputBuffers![0],
            scale!,
            inputBuffers![0].length
          );
        }

        const response: NasoWorkerResponse = {
          type: 'result',
          payload: { outputBuffers },
        };
        self.postMessage(response, outputBuffers!.map(b => b.buffer));
      } catch (error) {
        const response: NasoWorkerResponse = {
          type: 'error',
          payload: { error: error instanceof Error ? error.message : String(error) },
        };
        self.postMessage(response);
      }
      break;
    }

    case 'stream': {
      // Handle streaming token generation
      const { chunk } = payload!;
      if (streamRingBuffer && chunk) {
        const encoder = new TextEncoder();
        const data = encoder.encode(chunk);
        const view = new Uint8Array(streamRingBuffer);
        
        // Write to ring buffer
        for (let i = 0; i < data.length; i++) {
          view[streamWriteIndex] = data[i];
          streamWriteIndex = (streamWriteIndex + 1) % view.length;
        }

        const response: NasoWorkerResponse = {
          type: 'stream-chunk',
          payload: { chunk },
        };
        self.postMessage(response);
      }
      break;
    }

    case 'terminate': {
      // Cleanup
      for (const [, buffer] of gpuBuffers) {
        buffer.destroy();
      }
      gpuBuffers.clear();
      computePipelines.clear();
      gpuDevice = null;
      nasoWasmModule = null;
      nasoWasmInstance = null;
      wasmMemory = null;
      streamRingBuffer = null;
      break;
    }
  }
};

// Export types for TypeScript
export type { NasoWorkerMessage, NasoWorkerResponse };