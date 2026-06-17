/**
 * Task 0.2 — Throwaway spike: prove WebGPU is available in WebView2.
 * Returns a status string that is displayed prominently in App.svelte.
 */
export async function runTriangle(canvas: HTMLCanvasElement): Promise<string> {
  if (!navigator.gpu) {
    const gl = canvas.getContext("webgl2");
    return gl ? "NO_WEBGPU_BUT_WEBGL2" : "NO_WEBGPU_NO_WEBGL2";
  }
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) return "NO_ADAPTER";
  const device = await adapter.requestDevice();
  const ctx = canvas.getContext("webgpu")!;
  const format = navigator.gpu.getPreferredCanvasFormat();
  ctx.configure({ device, format, alphaMode: "opaque" });
  const shader = device.createShaderModule({
    code: `
    @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
      var p = array(vec2(0.0,0.5), vec2(-0.5,-0.5), vec2(0.5,-0.5));
      return vec4(p[i], 0.0, 1.0);
    }
    @fragment fn fs() -> @location(0) vec4<f32> { return vec4(0.2,0.8,1.0,1.0); }`,
  });
  const pipeline = device.createRenderPipeline({
    layout: "auto",
    vertex: { module: shader, entryPoint: "vs" },
    fragment: { module: shader, entryPoint: "fs", targets: [{ format }] },
    primitive: { topology: "triangle-list" },
  });
  const enc = device.createCommandEncoder();
  const pass = enc.beginRenderPass({
    colorAttachments: [
      {
        view: ctx.getCurrentTexture().createView(),
        clearValue: { r: 0, g: 0, b: 0, a: 1 },
        loadOp: "clear",
        storeOp: "store",
      },
    ],
  });
  pass.setPipeline(pipeline);
  pass.draw(3);
  pass.end();
  device.queue.submit([enc.finish()]);
  return "WEBGPU_OK";
}
