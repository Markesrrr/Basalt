import { drawSkinPreview, type SkinViewMode } from "./skin_canvas";

const VS_SOURCE = `
attribute vec3 aPos;
attribute vec3 aNorm;
attribute vec2 aUv;

uniform mat4 uMvp;
uniform mat4 uModel;

varying vec3 vWorldPos;
varying vec3 vWorldNorm;
varying vec2 vUv;

void main() {
  vec4 worldPos = uModel * vec4(aPos, 1.0);
  gl_Position = uMvp * vec4(aPos, 1.0);
  vWorldPos = worldPos.xyz;
  vWorldNorm = mat3(uModel) * aNorm;
  vUv = aUv;
}
`;

const FS_SOURCE = `
precision mediump float;

uniform sampler2D uTex;
uniform float uShadow;
uniform float uAlphaCutoff;
uniform float uIsShadow;

varying vec3 vWorldPos;
varying vec3 vWorldNorm;
varying vec2 vUv;

void main() {
  if (uIsShadow > 0.5) {
    vec2 d = (vUv - 0.5) * 2.0;
    float r2 = dot(d, d);
    if (r2 > 1.0) discard;
    float core = exp(-r2 * 5.2) * 0.34;
    float penumbra = pow(1.0 - r2, 2.3) * 0.16;
    float alpha = (core + penumbra) * uShadow;
    gl_FragColor = vec4(0.02, 0.03, 0.05, alpha);
    return;
  }

  vec4 tex = texture2D(uTex, vUv);
  if (tex.a < uAlphaCutoff) discard;

  vec3 n = normalize(vWorldNorm);
  vec3 viewDir = normalize(vec3(0.0, 16.4, 72.0) - vWorldPos);
  vec3 keyDir = normalize(vec3(0.42, 0.74, 0.84));
  vec3 fillDir = normalize(vec3(-0.62, 0.28, -0.42));

  float key = max(dot(n, keyDir), 0.0);
  float fill = max(dot(n, fillDir), 0.0);

  // Subtle fresnel rim-light biased toward upper silhouette for crisp Dark/Light separation
  float fresnel = pow(1.0 - max(dot(n, viewDir), 0.0), 2.75);
  float rimMask = smoothstep(-0.32, 0.85, n.y * 0.65 - n.z * 0.25 + 0.35);
  float rim = fresnel * rimMask * 0.22;

  // Ground contact ambient occlusion near the feet
  float footAo = mix(0.84, 1.0, smoothstep(0.0, 7.2, vWorldPos.y));

  float light = (0.55 + key * 0.43 + fill * 0.14) * footAo;
  vec3 rgb = tex.rgb * clamp(light, 0.48, 1.06) * uShadow;
  rgb += mix(tex.rgb, vec3(1.0, 0.97, 0.93), 0.55) * rim;
  gl_FragColor = vec4(min(rgb, vec3(1.0)), tex.a);
}
`;

type Mat4 = Float32Array;

function mat4Identity(): Mat4 {
  const m = new Float32Array(16);
  m[0] = 1;
  m[5] = 1;
  m[10] = 1;
  m[15] = 1;
  return m;
}

function mat4Multiply(a: Mat4, b: Mat4): Mat4 {
  const out = new Float32Array(16);
  for (let col = 0; col < 4; col += 1) {
    const b0 = b[col * 4];
    const b1 = b[col * 4 + 1];
    const b2 = b[col * 4 + 2];
    const b3 = b[col * 4 + 3];
    out[col * 4] = a[0] * b0 + a[4] * b1 + a[8] * b2 + a[12] * b3;
    out[col * 4 + 1] = a[1] * b0 + a[5] * b1 + a[9] * b2 + a[13] * b3;
    out[col * 4 + 2] = a[2] * b0 + a[6] * b1 + a[10] * b2 + a[14] * b3;
    out[col * 4 + 3] = a[3] * b0 + a[7] * b1 + a[11] * b2 + a[15] * b3;
  }
  return out;
}

function mat4Perspective(fovRad: number, aspect: number, near: number, far: number): Mat4 {
  const f = 1.0 / Math.tan(fovRad / 2);
  const nf = 1 / (near - far);
  const out = new Float32Array(16);
  out[0] = f / aspect;
  out[5] = f;
  out[10] = (far + near) * nf;
  out[11] = -1;
  out[14] = 2 * far * near * nf;
  return out;
}

function mat4Translate(x: number, y: number, z: number): Mat4 {
  const m = mat4Identity();
  m[12] = x;
  m[13] = y;
  m[14] = z;
  return m;
}

function mat4RotateX(rad: number): Mat4 {
  const m = mat4Identity();
  const c = Math.cos(rad);
  const s = Math.sin(rad);
  m[5] = c;
  m[6] = s;
  m[9] = -s;
  m[10] = c;
  return m;
}

function mat4RotateY(rad: number): Mat4 {
  const m = mat4Identity();
  const c = Math.cos(rad);
  const s = Math.sin(rad);
  m[0] = c;
  m[2] = -s;
  m[8] = s;
  m[10] = c;
  return m;
}

function mat4RotateZ(rad: number): Mat4 {
  const m = mat4Identity();
  const c = Math.cos(rad);
  const s = Math.sin(rad);
  m[0] = c;
  m[1] = s;
  m[4] = -s;
  m[5] = c;
  return m;
}

interface PartMesh {
  buf: WebGLBuffer;
  count: number;
  overlay: boolean;
}

/**
 * Builds vertex buffer `[px, py, pz, nx, ny, nz, u, v]` for a Minecraft box
 * centered at `(cx, cy, cz)` with size `(w, h, d)` and texture origin `(tu, tv)`.
 */
function buildMinecraftBoxData(
  w: number,
  h: number,
  d: number,
  cx: number,
  cy: number,
  cz: number,
  tu: number,
  tv: number,
  texW: number,
  texH: number,
  inflate: number
): Float32Array {
  const hw = w / 2 + inflate;
  const hh = h / 2 + inflate;
  const hd = d / 2 + inflate;

  const x0 = cx - hw;
  const x1 = cx + hw;
  const y0 = cy - hh;
  const y1 = cy + hh;
  const z0 = cz - hd;
  const z1 = cz + hd;

  const verts: number[] = [];
  const pushQuad = (
    p0: [number, number, number],
    p1: [number, number, number],
    p2: [number, number, number],
    p3: [number, number, number],
    n: [number, number, number],
    px: number,
    py: number,
    pw: number,
    ph: number
  ) => {
    const u0 = (px + 0.001) / texW;
    const u1 = (px + pw - 0.001) / texW;
    const v0 = (py + 0.001) / texH;
    const v1 = (py + ph - 0.001) / texH;

    // 2 triangles: p0-p2-p3, p0-p3-p1 (CCW facing outward)
    verts.push(
      p0[0], p0[1], p0[2], n[0], n[1], n[2], u0, v0,
      p2[0], p2[1], p2[2], n[0], n[1], n[2], u0, v1,
      p3[0], p3[1], p3[2], n[0], n[1], n[2], u1, v1,
      p0[0], p0[1], p0[2], n[0], n[1], n[2], u0, v0,
      p3[0], p3[1], p3[2], n[0], n[1], n[2], u1, v1,
      p1[0], p1[1], p1[2], n[0], n[1], n[2], u1, v0
    );
  };

  // Front (+Z)
  pushQuad([x0, y1, z1], [x1, y1, z1], [x0, y0, z1], [x1, y0, z1], [0, 0, 1], tu + d, tv + d, w, h);
  // Back (-Z)
  pushQuad([x1, y1, z0], [x0, y1, z0], [x1, y0, z0], [x0, y0, z0], [0, 0, -1], tu + d + w + d, tv + d, w, h);
  // Right (-X, character's right / viewer's left)
  pushQuad([x0, y1, z0], [x0, y1, z1], [x0, y0, z0], [x0, y0, z1], [-1, 0, 0], tu, tv + d, d, h);
  // Left (+X, character's left / viewer's right)
  pushQuad([x1, y1, z1], [x1, y1, z0], [x1, y0, z1], [x1, y0, z0], [1, 0, 0], tu + d + w, tv + d, d, h);
  // Top (+Y)
  pushQuad([x0, y1, z0], [x1, y1, z0], [x0, y1, z1], [x1, y1, z1], [0, 1, 0], tu + d, tv, w, d);
  // Bottom (-Y)
  pushQuad([x0, y0, z1], [x1, y0, z1], [x0, y0, z0], [x1, y0, z0], [0, -1, 0], tu + d + w, tv, w, d);

  return new Float32Array(verts);
}

/**
 * Builds a flat ground-plane quad for the soft radial contact shadow underneath the feet.
 */
function buildGroundShadowData(rx: number, rz: number, y: number): Float32Array {
  return new Float32Array([
    -rx, y, -rz, 0, 1, 0, 0, 0,
    -rx, y,  rz, 0, 1, 0, 0, 1,
     rx, y,  rz, 0, 1, 0, 1, 1,
    -rx, y, -rz, 0, 1, 0, 0, 0,
     rx, y,  rz, 0, 1, 0, 1, 1,
     rx, y, -rz, 0, 1, 0, 1, 0,
  ]);
}

function compileShader(gl: WebGLRenderingContext, type: number, src: string): WebGLShader | null {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    gl.deleteShader(shader);
    return null;
  }
  return shader;
}

function createProgram(gl: WebGLRenderingContext): WebGLProgram | null {
  const vs = compileShader(gl, gl.VERTEX_SHADER, VS_SOURCE);
  const fs = compileShader(gl, gl.FRAGMENT_SHADER, FS_SOURCE);
  if (!vs || !fs) return null;
  const prog = gl.createProgram();
  if (!prog) return null;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    gl.deleteProgram(prog);
    return null;
  }
  return prog;
}

export interface Skin3DOption {
  interactive?: boolean;
  trackCursor?: boolean;
  initialView?: "front" | "back";
  onClickFlip?: (mode: "front" | "back") => void;
  onResetPose?: () => void;
}

export interface Skin3DPose {
  yaw: number;
  pitch: number;
  view: "front" | "back";
}

const DEFAULT_FRONT_YAW = 0.24;
const DEFAULT_PITCH = 0.06;
const MIN_PITCH = -0.42;
const MAX_PITCH = 0.45;
const INERTIA_DAMPING = 5.4;
const MAX_YAW_VEL = 13.5;
const MAX_PITCH_VEL = 5.5;

let pointerInitialized = false;
let pointerX = 0;
let pointerY = 0;
let hasPointerMoved = false;

function ensureGlobalPointerTracking(): void {
  if (pointerInitialized) return;
  pointerInitialized = true;
  pointerX = window.innerWidth * 0.5;
  pointerY = window.innerHeight * 0.4;
  window.addEventListener(
    "pointermove",
    (e) => {
      pointerX = e.clientX;
      pointerY = e.clientY;
      hasPointerMoved = true;
    },
    { passive: true }
  );
}

function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, v));
}

export class Skin3DViewer {
  private canvas: HTMLCanvasElement;
  private gl: WebGLRenderingContext | null = null;
  private prog: WebGLProgram | null = null;
  private tex: WebGLTexture | null = null;
  private meshes: Record<string, PartMesh[]> = {};
  private groundShadowMesh: PartMesh | null = null;
  private locPos = -1;
  private locNorm = -1;
  private locUv = -1;
  private locMvp: WebGLUniformLocation | null = null;
  private locModel: WebGLUniformLocation | null = null;
  private locTex: WebGLUniformLocation | null = null;
  private locShadow: WebGLUniformLocation | null = null;
  private locCutoff: WebGLUniformLocation | null = null;
  private locIsShadow: WebGLUniformLocation | null = null;

  private rgba: number[] = [];
  private isSlim = true;
  private hasSkinTex = false;

  private trackCursor: boolean;
  private reducedMotion = false;
  private currentView: "front" | "back" = "front";
  private baseYaw = DEFAULT_FRONT_YAW;
  private targetBaseYaw = DEFAULT_FRONT_YAW;
  private basePitch = DEFAULT_PITCH;
  private targetBasePitch = DEFAULT_PITCH;
  private lookYaw = 0;
  private lookPitch = 0;

  // Damped drag-to-orbit inertia state (radians / sec)
  private velYaw = 0;
  private velPitch = 0;
  private dragging = false;
  private dragStartX = 0;
  private dragStartY = 0;
  private dragDist = 0;
  private lastDragTime = 0;
  private lastTapTime = 0;
  private onClickFlipCb?: (mode: "front" | "back") => void;
  private onResetPoseCb?: () => void;

  private lastTick = performance.now();
  private rafId: number | null = null;
  private fallback2dMode = false;
  private cachedRect = { left: 0, top: 0, width: 240, height: 280 };
  private resizeObs: ResizeObserver | null = null;
  private onVisibilityChange: (() => void) | null = null;
  private listenerAbort: AbortController | null = null;

  constructor(canvas: HTMLCanvasElement, opts: Skin3DOption = {}) {
    this.canvas = canvas;
    this.trackCursor = opts.trackCursor ?? true;
    this.currentView = opts.initialView ?? "front";
    this.baseYaw = this.currentView === "back" ? Math.PI - DEFAULT_FRONT_YAW : DEFAULT_FRONT_YAW;
    this.targetBaseYaw = this.baseYaw;
    this.onClickFlipCb = opts.onClickFlip;
    this.onResetPoseCb = opts.onResetPose;

    ensureGlobalPointerTracking();
    this.listenerAbort = new AbortController();
    this.initGl();
    this.observeBounds();

    this.canvas.addEventListener(
      "webglcontextlost",
      (e) => {
        e.preventDefault();
        this.gl = null;
        this.prog = null;
        this.tex = null;
        this.groundShadowMesh = null;
        this.fallback2dMode = true;
        if (this.rafId !== null) {
          cancelAnimationFrame(this.rafId);
          this.rafId = null;
        }
        if (this.rgba.length) {
          drawSkinPreview(this.canvas, this.rgba, this.currentView as SkinViewMode, this.isSlim);
        }
      },
      { signal: this.listenerAbort.signal }
    );
    this.canvas.addEventListener(
      "webglcontextrestored",
      () => {
        this.fallback2dMode = false;
        this.initGl();
        if (!this.fallback2dMode && this.rgba.length) {
          this.setSkin(this.rgba, this.isSlim);
        }
      },
      { signal: this.listenerAbort.signal }
    );

    if (opts.interactive ?? true) {
      this.bindInteraction();
    }
    this.onVisibilityChange = () => {
      if (document.hidden) {
        if (this.rafId !== null) {
          cancelAnimationFrame(this.rafId);
          this.rafId = null;
        }
      } else {
        this.lastTick = performance.now();
        this.startLoop();
      }
    };
    document.addEventListener("visibilitychange", this.onVisibilityChange);
    this.startLoop();
  }

  private refreshCachedBounds(): void {
    const rect = this.canvas.getBoundingClientRect();
    if (rect.width >= 10 && rect.height >= 10) {
      this.cachedRect = {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
      };
    }
  }

  private observeBounds(): void {
    const signal = this.listenerAbort?.signal;
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObs = new ResizeObserver((entries) => {
        const entry = entries[0];
        if (!entry) return;
        const { width, height } = entry.contentRect;
        if (width >= 10 && height >= 10) {
          this.refreshCachedBounds();
          this.resizeCanvas();
          if (this.fallback2dMode && this.rgba.length) {
            drawSkinPreview(this.canvas, this.rgba, this.currentView as SkinViewMode, this.isSlim);
          } else {
            this.wake();
          }
        }
      });
      this.resizeObs.observe(this.canvas);
    }
    window.addEventListener("resize", () => this.refreshCachedBounds(), { passive: true, signal });
    window.addEventListener(
      "pointermove",
      () => {
        if (this.trackCursor && !this.reducedMotion) this.wake();
      },
      { passive: true, signal }
    );
  }

  private initGl(): void {
    const gl = this.canvas.getContext("webgl", {
      alpha: true,
      antialias: true,
      premultipliedAlpha: false,
      powerPreference: "high-performance",
    });
    if (!gl) {
      this.fallback2dMode = true;
      return;
    }
    const prog = createProgram(gl);
    if (!prog) {
      this.fallback2dMode = true;
      return;
    }
    const tex = gl.createTexture();
    if (!tex) {
      this.fallback2dMode = true;
      return;
    }
    this.gl = gl;
    this.prog = prog;
    this.locPos = gl.getAttribLocation(prog, "aPos");
    this.locNorm = gl.getAttribLocation(prog, "aNorm");
    this.locUv = gl.getAttribLocation(prog, "aUv");
    this.locMvp = gl.getUniformLocation(prog, "uMvp");
    this.locModel = gl.getUniformLocation(prog, "uModel");
    this.locTex = gl.getUniformLocation(prog, "uTex");
    this.locShadow = gl.getUniformLocation(prog, "uShadow");
    this.locCutoff = gl.getUniformLocation(prog, "uAlphaCutoff");
    this.locIsShadow = gl.getUniformLocation(prog, "uIsShadow");

    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    this.tex = tex;

    this.rebuildGeometry(this.isSlim);
  }

  private createPartBuffer(data: Float32Array, overlay: boolean): PartMesh | null {
    if (!this.gl) return null;
    const buf = this.gl.createBuffer();
    if (!buf) return null;
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, buf);
    this.gl.bufferData(this.gl.ARRAY_BUFFER, data, this.gl.STATIC_DRAW);
    return {
      buf,
      count: data.length / 8,
      overlay,
    };
  }

  private rebuildGeometry(isSlim: boolean): void {
    const gl = this.gl;
    if (!gl) return;
    for (const group of Object.values(this.meshes)) {
      for (const part of group) {
        gl.deleteBuffer(part.buf);
      }
    }
    if (this.groundShadowMesh) {
      gl.deleteBuffer(this.groundShadowMesh.buf);
      this.groundShadowMesh = null;
    }

    const armW = isSlim ? 3 : 4;
    const addGroup = (name: string, base: Float32Array, overlay: Float32Array) => {
      const m1 = this.createPartBuffer(base, false);
      const m2 = this.createPartBuffer(overlay, true);
      this.meshes[name] = [m1, m2].filter((x): x is PartMesh => x !== null);
    };

    // Head: pivot at neck (0, 24, 0), cube center at (0, 4, 0)
    addGroup(
      "head",
      buildMinecraftBoxData(8, 8, 8, 0, 4, 0, 0, 0, 64, 64, 0),
      buildMinecraftBoxData(8, 8, 8, 0, 4, 0, 32, 0, 64, 64, 0.48)
    );
    // Torso: pivot at hips (0, 12, 0), cube center at (0, 6, 0)
    addGroup(
      "torso",
      buildMinecraftBoxData(8, 12, 4, 0, 6, 0, 16, 16, 64, 64, 0),
      buildMinecraftBoxData(8, 12, 4, 0, 6, 0, 16, 32, 64, 64, 0.28)
    );
    // Right Arm: shoulder pivot (-4 - armW/2, 22, 0), cube center (0, -4, 0)
    addGroup(
      "rightArm",
      buildMinecraftBoxData(armW, 12, 4, 0, -4, 0, 40, 16, 64, 64, 0),
      buildMinecraftBoxData(armW, 12, 4, 0, -4, 0, 40, 32, 64, 64, 0.28)
    );
    // Left Arm: shoulder pivot (+4 + armW/2, 22, 0), cube center (0, -4, 0)
    addGroup(
      "leftArm",
      buildMinecraftBoxData(armW, 12, 4, 0, -4, 0, 32, 48, 64, 64, 0),
      buildMinecraftBoxData(armW, 12, 4, 0, -4, 0, 48, 48, 64, 64, 0.28)
    );
    // Right Leg: hip pivot (-2, 12, 0), cube center (0, -6, 0)
    addGroup(
      "rightLeg",
      buildMinecraftBoxData(4, 12, 4, 0, -6, 0, 0, 16, 64, 64, 0),
      buildMinecraftBoxData(4, 12, 4, 0, -6, 0, 0, 32, 64, 64, 0.28)
    );
    // Left Leg: hip pivot (+2, 12, 0), cube center (0, -6, 0)
    addGroup(
      "leftLeg",
      buildMinecraftBoxData(4, 12, 4, 0, -6, 0, 16, 48, 64, 64, 0),
      buildMinecraftBoxData(4, 12, 4, 0, -6, 0, 0, 48, 64, 64, 0.28)
    );

    // Soft ground contact shadow disc right below feet (y = -0.12)
    this.groundShadowMesh = this.createPartBuffer(buildGroundShadowData(11.5, 9.5, -0.12), false);
  }

  private bindInteraction(): void {
    const signal = this.listenerAbort?.signal;
    this.canvas.style.cursor = "grab";

    this.canvas.addEventListener(
      "pointerdown",
      (e) => {
        if (e.button !== 0) return;
        this.dragging = true;
        this.dragStartX = e.clientX;
        this.dragStartY = e.clientY;
        this.dragDist = 0;
        this.velYaw = 0;
        this.velPitch = 0;
        this.lastDragTime = performance.now();
        this.canvas.style.cursor = "grabbing";
        try {
          this.canvas.setPointerCapture(e.pointerId);
        } catch {
          // Synthetic events ignore capture
        }
        this.wake();
      },
      { signal }
    );

    window.addEventListener(
      "pointermove",
      (e) => {
        if (!this.dragging) return;
        const now = performance.now();
        const dx = e.clientX - this.dragStartX;
        const dy = e.clientY - this.dragStartY;
        const dtSec = Math.max(0.006, (now - this.lastDragTime) / 1000);
        this.dragStartX = e.clientX;
        this.dragStartY = e.clientY;
        this.lastDragTime = now;
        this.dragDist += Math.hypot(dx, dy);

        const dYaw = dx * 0.013;
        const dPitch = dy * 0.008;
        this.targetBaseYaw += dYaw;
        this.targetBasePitch = clamp(this.targetBasePitch + dPitch, MIN_PITCH, MAX_PITCH);

        if (!this.reducedMotion) {
          const instYaw = clamp(dYaw / dtSec, -MAX_YAW_VEL, MAX_YAW_VEL);
          const instPitch = clamp(dPitch / dtSec, -MAX_PITCH_VEL, MAX_PITCH_VEL);
          this.velYaw = this.velYaw * 0.35 + instYaw * 0.65;
          this.velPitch = this.velPitch * 0.35 + instPitch * 0.65;
        }
        this.wake();
      },
      { signal }
    );

    const stopDrag = (e: PointerEvent) => {
      if (!this.dragging) return;
      this.dragging = false;
      this.canvas.style.cursor = "grab";
      try {
        if (this.canvas.hasPointerCapture(e.pointerId)) {
          this.canvas.releasePointerCapture(e.pointerId);
        }
      } catch {
        // Ignore
      }

      const now = performance.now();
      if (this.dragDist < 5) {
        this.velYaw = 0;
        this.velPitch = 0;
        if (now - this.lastTapTime < 320) {
          // Rapid double-tap / double-click resets pose cleanly
          this.lastTapTime = 0;
          this.resetPose();
        } else {
          this.lastTapTime = now;
          const next: "front" | "back" = this.currentView === "front" ? "back" : "front";
          this.setPresetView(next);
          this.onClickFlipCb?.(next);
        }
        return;
      }

      // If pointer rested before releasing, or reduced motion is active, suppress fling inertia
      if (this.reducedMotion || now - this.lastDragTime > 85) {
        this.velYaw = 0;
        this.velPitch = 0;
      } else {
        // Update logical facing indicator after manual orbit
        const normCos = Math.cos(this.targetBaseYaw);
        this.currentView = normCos >= 0 ? "front" : "back";
      }
      this.wake();
    };

    window.addEventListener("pointerup", stopDrag, { signal });
    window.addEventListener("pointercancel", stopDrag, { signal });

    this.canvas.addEventListener(
      "dblclick",
      (e) => {
        e.preventDefault();
        this.lastTapTime = 0;
        this.resetPose();
      },
      { signal }
    );
  }

  public wake(): void {
    if (this.fallback2dMode || document.hidden || this.rafId !== null) return;
    this.lastTick = performance.now();
    this.startLoop();
  }

  public setSkin(rgba: number[], isSlim: boolean): void {
    this.rgba = rgba;
    const slimChanged = isSlim !== this.isSlim;
    this.isSlim = isSlim;

    if (this.fallback2dMode) {
      drawSkinPreview(this.canvas, rgba, this.currentView as SkinViewMode, isSlim);
      return;
    }

    if (slimChanged) {
      this.rebuildGeometry(isSlim);
    }

    const gl = this.gl;
    if (!gl || !this.tex || rgba.length !== 64 * 64 * 4) return;
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA,
      64,
      64,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      new Uint8Array(rgba)
    );
    this.hasSkinTex = true;
    this.refreshCachedBounds();
    this.wake();
  }

  public setPresetView(mode: "front" | "back"): void {
    this.currentView = mode;
    this.velYaw = 0;
    this.velPitch = 0;
    const offset = mode === "back" ? Math.PI - DEFAULT_FRONT_YAW : DEFAULT_FRONT_YAW;
    const turns = Math.round((this.targetBaseYaw - offset) / (Math.PI * 2));
    this.targetBaseYaw = turns * Math.PI * 2 + offset;
    this.targetBasePitch = DEFAULT_PITCH;
    if (this.fallback2dMode && this.rgba.length) {
      drawSkinPreview(this.canvas, this.rgba, mode, this.isSlim);
    } else {
      this.wake();
    }
  }

  /**
   * Smoothly resets the character orbit, pitch, and velocity back to the default hero pose.
   */
  public resetPose(): void {
    this.currentView = "front";
    this.velYaw = 0;
    this.velPitch = 0;
    const turns = Math.round((this.targetBaseYaw - DEFAULT_FRONT_YAW) / (Math.PI * 2));
    this.targetBaseYaw = turns * Math.PI * 2 + DEFAULT_FRONT_YAW;
    this.targetBasePitch = DEFAULT_PITCH;
    if (this.reducedMotion) {
      this.baseYaw = this.targetBaseYaw;
      this.basePitch = this.targetBasePitch;
      this.lookYaw = 0;
      this.lookPitch = 0;
    }
    this.onClickFlipCb?.("front");
    this.onResetPoseCb?.();
    if (this.fallback2dMode && this.rgba.length) {
      drawSkinPreview(this.canvas, this.rgba, "front", this.isSlim);
    } else {
      this.wake();
    }
  }

  public getPose(): Skin3DPose {
    return {
      yaw: this.baseYaw,
      pitch: this.basePitch,
      view: this.currentView,
    };
  }

  public setReducedMotion(reduced: boolean): void {
    this.reducedMotion = reduced;
    if (reduced) {
      this.velYaw = 0;
      this.velPitch = 0;
      this.baseYaw = this.targetBaseYaw;
      this.basePitch = this.targetBasePitch;
      this.lookYaw = 0;
      this.lookPitch = 0;
    }
    this.wake();
  }

  private resizeCanvas(): void {
    const { width, height } = this.cachedRect;
    if (width < 10 || height < 10) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const targetW = Math.round(width * dpr);
    const targetH = Math.round(height * dpr);
    if (this.canvas.width !== targetW || this.canvas.height !== targetH) {
      this.canvas.width = targetW;
      this.canvas.height = targetH;
    }
  }

  private startLoop(): void {
    if (this.rafId !== null || document.hidden || this.fallback2dMode) return;
    const frame = (now: number) => {
      this.rafId = null;
      if (document.hidden || this.fallback2dMode || !this.hasSkinTex) return;
      if (this.canvas.offsetParent === null) {
        return;
      }
      const keepGoing = this.renderFrame(now);
      if (keepGoing) {
        this.rafId = requestAnimationFrame(frame);
      }
    };
    this.rafId = requestAnimationFrame(frame);
  }

  private renderFrame(now: number): boolean {
    const gl = this.gl;
    const prog = this.prog;
    if (!gl || !prog || !this.tex) return false;

    const dt = Math.min(0.05, Math.max(0.001, (now - this.lastTick) / 1000));
    this.lastTick = now;

    this.resizeCanvas();
    const w = this.canvas.width;
    const h = this.canvas.height;
    if (w === 0 || h === 0) return false;

    // Damped drag-to-orbit inertia after pointer release
    if (!this.dragging && !this.reducedMotion) {
      if (Math.abs(this.velYaw) > 0.01 || Math.abs(this.velPitch) > 0.01) {
        this.targetBaseYaw += this.velYaw * dt;
        const nextPitch = this.targetBasePitch + this.velPitch * dt;
        if (nextPitch <= MIN_PITCH || nextPitch >= MAX_PITCH) {
          this.targetBasePitch = clamp(nextPitch, MIN_PITCH, MAX_PITCH);
          this.velPitch *= -0.25;
        } else {
          this.targetBasePitch = nextPitch;
        }
        const damp = Math.exp(-INERTIA_DAMPING * dt);
        this.velYaw *= damp;
        this.velPitch *= damp;
        if (Math.abs(this.velYaw) < 0.01) this.velYaw = 0;
        if (Math.abs(this.velPitch) < 0.01) this.velPitch = 0;
        this.currentView = Math.cos(this.targetBaseYaw) >= 0 ? "front" : "back";
      }
    }

    const smooth = this.reducedMotion ? 1 : Math.min(1, dt * 12);
    this.baseYaw += (this.targetBaseYaw - this.baseYaw) * smooth;
    this.basePitch += (this.targetBasePitch - this.basePitch) * smooth;

    let targetLookYaw = 0;
    let targetLookPitch = 0;
    if (this.trackCursor && hasPointerMoved && !this.reducedMotion) {
      const cx = this.cachedRect.left + this.cachedRect.width * 0.5;
      const cy = this.cachedRect.top + this.cachedRect.height * 0.34;
      const spanX = Math.max(window.innerWidth * 0.38, 280);
      const spanY = Math.max(window.innerHeight * 0.36, 240);
      const nx = clamp((pointerX - cx) / spanX, -1, 1);
      const ny = clamp((pointerY - cy) / spanY, -1, 1);
      const facingFront = (Math.cos(this.baseYaw) + 1) * 0.5;
      const followGain = 0.25 + 0.75 * facingFront;
      targetLookYaw = nx * 0.66 * followGain;
      targetLookPitch = ny * 0.42 * followGain;
    }

    const lookLerp = this.reducedMotion ? 1 : Math.min(1, dt * 9.5);
    this.lookYaw += (targetLookYaw - this.lookYaw) * lookLerp;
    this.lookPitch += (targetLookPitch - this.lookPitch) * lookLerp;

    // Subtle idle breathing & relaxed cape-less micro-pose (strictly disabled in reducedMotion)
    const idlePhase = this.reducedMotion ? 0 : now * 0.00155;
    const breathWave = this.reducedMotion ? 0 : Math.sin(idlePhase);
    const armBreath = this.reducedMotion ? 0.036 : 0.042 + breathWave * 0.026;
    const armSwing = this.reducedMotion ? 0 : Math.sin(idlePhase * 0.78) * 0.044;
    const bodyBob = this.reducedMotion ? 0 : Math.sin(idlePhase * 1.15) * 0.14;
    const torsoTwist = this.reducedMotion ? 0 : Math.sin(idlePhase * 0.58 + 0.5) * 0.028;
    const headTilt = this.reducedMotion ? 0 : Math.sin(idlePhase * 0.48 + 1.1) * 0.02;
    const stanceShift = this.reducedMotion ? 0.018 : 0.024 + Math.sin(idlePhase * 0.78) * 0.016;

    gl.viewport(0, 0, w, h);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.enable(gl.DEPTH_TEST);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);

    gl.useProgram(prog);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.uniform1i(this.locTex, 0);

    const proj = mat4Perspective(0.54, w / h, 1.0, 200.0);
    const cam = mat4Translate(0, -16.4, -72.0);
    const view = mat4Multiply(proj, cam);
    const orbitRot = mat4Multiply(
      mat4RotateX(this.basePitch),
      mat4RotateY(this.baseYaw + this.lookYaw * 0.14)
    );
    const rootModel = mat4Multiply(mat4Translate(0, bodyBob, 0), orbitRot);

    const bindAndDraw = (mesh: PartMesh, mvp: Mat4, model: Mat4, shadow: number, isGroundShadow: boolean) => {
      gl.bindBuffer(gl.ARRAY_BUFFER, mesh.buf);
      gl.enableVertexAttribArray(this.locPos);
      gl.vertexAttribPointer(this.locPos, 3, gl.FLOAT, false, 32, 0);
      gl.enableVertexAttribArray(this.locNorm);
      gl.vertexAttribPointer(this.locNorm, 3, gl.FLOAT, false, 32, 12);
      gl.enableVertexAttribArray(this.locUv);
      gl.vertexAttribPointer(this.locUv, 2, gl.FLOAT, false, 32, 24);
      gl.uniformMatrix4fv(this.locMvp, false, mvp);
      gl.uniformMatrix4fv(this.locModel, false, model);
      gl.uniform1f(this.locShadow, shadow);
      gl.uniform1f(this.locCutoff, mesh.overlay ? 0.08 : 0.5);
      gl.uniform1f(this.locIsShadow, isGroundShadow ? 1.0 : 0.0);
      gl.drawArrays(gl.TRIANGLES, 0, mesh.count);
    };

    // Pass 0: Crisp soft ground contact shadow anchored directly beneath the character
    if (this.groundShadowMesh) {
      gl.disable(gl.CULL_FACE);
      gl.depthMask(false);
      const shadowStrength = clamp(1.0 - bodyBob * 0.25, 0.85, 1.05);
      const shadowMvp = mat4Multiply(view, orbitRot);
      bindAndDraw(this.groundShadowMesh, shadowMvp, orbitRot, shadowStrength, true);
      gl.depthMask(true);
    }

    const armOffsetX = 4 + (this.isSlim ? 1.5 : 2);
    const partTransforms: Array<{ name: string; local: Mat4; shadow: number }> = [
      {
        name: "torso",
        local: mat4Multiply(mat4Translate(0, 12, 0), mat4RotateY(this.lookYaw * 0.18 + torsoTwist)),
        shadow: 1.0,
      },
      {
        name: "head",
        local: mat4Multiply(
          mat4Translate(0, 24 + (this.reducedMotion ? 0 : breathWave * 0.06), 0),
          mat4Multiply(
            mat4RotateY(this.lookYaw * 0.82 + torsoTwist * 0.4),
            mat4Multiply(mat4RotateX(this.lookPitch), mat4RotateZ(headTilt))
          )
        ),
        shadow: 1.03,
      },
      {
        name: "rightArm",
        local: mat4Multiply(
          mat4Translate(-armOffsetX, 22 + (this.reducedMotion ? 0 : breathWave * 0.08), 0),
          mat4Multiply(mat4RotateZ(-armBreath), mat4RotateX(armSwing - this.lookPitch * 0.08))
        ),
        shadow: 0.98,
      },
      {
        name: "leftArm",
        local: mat4Multiply(
          mat4Translate(armOffsetX, 22 + (this.reducedMotion ? 0 : breathWave * 0.08), 0),
          mat4Multiply(mat4RotateZ(armBreath), mat4RotateX(-armSwing - this.lookPitch * 0.08))
        ),
        shadow: 0.98,
      },
      {
        name: "rightLeg",
        local: mat4Multiply(mat4Translate(-2, 12, 0), mat4RotateX(-stanceShift)),
        shadow: 0.96,
      },
      {
        name: "leftLeg",
        local: mat4Multiply(mat4Translate(2, 12, 0), mat4RotateX(stanceShift)),
        shadow: 0.96,
      },
    ];

    // Pass 1: Solid opaque body boxes with back-face culling
    gl.enable(gl.CULL_FACE);
    for (const pt of partTransforms) {
      const model = mat4Multiply(rootModel, pt.local);
      const mvp = mat4Multiply(view, model);
      for (const mesh of this.meshes[pt.name] ?? []) {
        if (!mesh.overlay) bindAndDraw(mesh, mvp, model, pt.shadow, false);
      }
    }

    // Pass 2: Outer translucent layer (hat, jacket, sleeves, pants) double-sided
    gl.disable(gl.CULL_FACE);
    for (const pt of partTransforms) {
      const model = mat4Multiply(rootModel, pt.local);
      const mvp = mat4Multiply(view, model);
      for (const mesh of this.meshes[pt.name] ?? []) {
        if (mesh.overlay) bindAndDraw(mesh, mvp, model, pt.shadow * 1.02, false);
      }
    }

    if (!this.reducedMotion || this.dragging) return true;
    const settling =
      Math.abs(this.targetBaseYaw - this.baseYaw) > 0.002 ||
      Math.abs(this.targetBasePitch - this.basePitch) > 0.002 ||
      Math.abs(targetLookYaw - this.lookYaw) > 0.002 ||
      Math.abs(targetLookPitch - this.lookPitch) > 0.002 ||
      Math.abs(this.velYaw) > 0.01 ||
      Math.abs(this.velPitch) > 0.01;
    return settling;
  }

  public dispose(): void {
    if (this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    if (this.gl && this.groundShadowMesh) {
      this.gl.deleteBuffer(this.groundShadowMesh.buf);
      this.groundShadowMesh = null;
    }
    this.resizeObs?.disconnect();
    this.listenerAbort?.abort();
    this.listenerAbort = null;
    if (this.onVisibilityChange) {
      document.removeEventListener("visibilitychange", this.onVisibilityChange);
      this.onVisibilityChange = null;
    }
  }
}
