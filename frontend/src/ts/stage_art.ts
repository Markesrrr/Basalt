const SVG_NS = "http://www.w3.org/2000/svg";

const FAR_COLUMNS = [
  195, 235, 175, 255, 215, 275, 185, 245, 285, 225,
  260, 190, 235, 205, 270, 215, 185, 245, 275, 200,
];

const BACK_COLUMNS = [
  150, 190, 120, 210, 170, 240, 140, 200, 260, 180,
  220, 130, 190, 160, 230, 170, 140, 200, 250, 150,
];

const FRONT_COLUMNS = [
  90, 130, 70, 150, 110, 80, 160, 100, 60, 140,
  120, 90, 170, 110, 80, 130, 100, 150, 70, 120,
];

// Strategic vertical mineral/magma seams nestled cleanly in deep basalt clefts
const MAGMA_SEAMS: Array<{ x: number; y: number; h: number }> = [
  { x: 180, y: 182, h: 138 },
  { x: 420, y: 158, h: 162 },
  { x: 660, y: 196, h: 124 },
  { x: 900, y: 166, h: 154 },
  { x: 1080, y: 188, h: 132 },
];

const parallaxCleanup = new WeakMap<SVGSVGElement, () => void>();

function svgEl<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number>
): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) {
    el.setAttribute(k, String(v));
  }
  return el;
}

function isReducedMotionActive(): boolean {
  if (typeof document === "undefined") return true;
  if (document.documentElement.getAttribute("data-motion") === "none") return true;
  if (document.body.classList.contains("reduce-motion")) return true;
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

export function seedHue(seed: string): number {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i += 1) {
    h ^= seed.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return (h >>> 0) % 360;
}

/**
 * Binds subtle, zero-idle-repaint pointer-parallax to the layered Basalt strata SVG.
 * Uses CSS custom properties (`--strata-px`, `--strata-py`) with compositor-only transforms.
 * Automatically yields to `data-motion="none"` and `prefers-reduced-motion: reduce`.
 */
export function bindStrataParallax(
  svg: SVGSVGElement,
  container?: HTMLElement | null
): () => void {
  parallaxCleanup.get(svg)?.();

  const host = container ?? svg.closest<HTMLElement>(".stage-hero") ?? svg.parentElement;
  if (!host) return () => {};

  const abort = new AbortController();
  let targetX = 0;
  let targetY = 0;
  let currX = 0;
  let currY = 0;
  let rafId = 0;

  const applyVars = (x: number, y: number) => {
    svg.style.setProperty("--strata-px", x.toFixed(4));
    svg.style.setProperty("--strata-py", y.toFixed(4));
  };

  const step = () => {
    rafId = 0;
    if (document.hidden || isReducedMotionActive()) {
      currX = 0;
      currY = 0;
      targetX = 0;
      targetY = 0;
      applyVars(0, 0);
      return;
    }

    currX += (targetX - currX) * 0.14;
    currY += (targetY - currY) * 0.14;
    applyVars(currX, currY);

    if (Math.abs(targetX - currX) > 0.004 || Math.abs(targetY - currY) > 0.004) {
      rafId = requestAnimationFrame(step);
    } else {
      currX = targetX;
      currY = targetY;
      applyVars(currX, currY);
    }
  };

  const schedule = () => {
    if (rafId || document.hidden) return;
    if (isReducedMotionActive()) {
      if (currX !== 0 || currY !== 0) {
        currX = 0;
        currY = 0;
        applyVars(0, 0);
      }
      return;
    }
    rafId = requestAnimationFrame(step);
  };

  host.addEventListener(
    "pointermove",
    (e: PointerEvent) => {
      if (isReducedMotionActive()) return;
      const rect = host.getBoundingClientRect();
      if (rect.width < 20 || rect.height < 20) return;
      const nx = ((e.clientX - rect.left) / rect.width) * 2 - 1;
      const ny = ((e.clientY - rect.top) / rect.height) * 2 - 1;
      targetX = Math.max(-1, Math.min(1, nx));
      targetY = Math.max(-1, Math.min(1, ny));
      schedule();
    },
    { passive: true, signal: abort.signal }
  );

  host.addEventListener(
    "pointerleave",
    () => {
      targetX = 0;
      targetY = 0;
      schedule();
    },
    { passive: true, signal: abort.signal }
  );

  const cleanup = () => {
    abort.abort();
    if (rafId) {
      cancelAnimationFrame(rafId);
      rafId = 0;
    }
    applyVars(0, 0);
    parallaxCleanup.delete(svg);
  };

  parallaxCleanup.set(svg, cleanup);
  return cleanup;
}

export function renderStrata(svg: SVGSVGElement): void {
  if (svg.childElementCount > 0) return;

  const defs = svgEl("defs", {});

  // Restrained bottom magma/mineral horizon glow
  const grad = svgEl("linearGradient", { id: "strata-magma", x1: 0, y1: 0, x2: 0, y2: 1 });
  grad.append(
    svgEl("stop", { offset: "0%", style: "stop-color: var(--accent); stop-opacity: 0" }),
    svgEl("stop", { offset: "68%", style: "stop-color: var(--accent); stop-opacity: .2" }),
    svgEl("stop", { offset: "100%", style: "stop-color: var(--accent); stop-opacity: .44" })
  );
  defs.appendChild(grad);

  // Subtle vertical mineral fissure gradient for magma seams
  const seamGrad = svgEl("linearGradient", { id: "strata-seam-grad", x1: 0, y1: 0, x2: 0, y2: 1 });
  seamGrad.append(
    svgEl("stop", { offset: "0%", style: "stop-color: var(--accent); stop-opacity: 0" }),
    svgEl("stop", { offset: "55%", style: "stop-color: var(--accent); stop-opacity: .28" }),
    svgEl("stop", { offset: "100%", style: "stop-color: var(--accent); stop-opacity: .68" })
  );
  defs.appendChild(seamGrad);

  svg.appendChild(defs);

  const frag = document.createDocumentFragment();

  // Layer 1: Distant basalt ridge horizon (atmospheric depth)
  const farLayer = svgEl("g", { class: "strata-layer strata-layer--far" });
  for (let i = 0; i < FAR_COLUMNS.length; i += 1) {
    const h = FAR_COLUMNS[i];
    const y = 320 - h;
    const x = i * 62 - 20;
    const col = svgEl("g", { class: "strata-col", style: `--i:${i}` });
    col.append(
      svgEl("rect", { class: "strata-far", x, y, width: 64, height: h }),
      svgEl("rect", { class: "strata-cap strata-cap-far", x, y, width: 64, height: 5 })
    );
    farLayer.appendChild(col);
  }
  frag.appendChild(farLayer);

  // Layer 2: Mid basalt columns with subtle mineral left-edge bevels
  const midLayer = svgEl("g", { class: "strata-layer strata-layer--mid" });
  for (let i = 0; i < BACK_COLUMNS.length; i += 1) {
    const h = BACK_COLUMNS[i];
    const y = 320 - h;
    const x = i * 60;
    const col = svgEl("g", { class: "strata-col", style: `--i:${i + 2}` });
    col.append(
      svgEl("rect", { class: "strata-back", x, y, width: 62, height: h }),
      svgEl("rect", { class: "strata-bevel", x, y: y + 7, width: 3, height: Math.max(0, h - 7) }),
      svgEl("rect", { class: "strata-cap strata-cap-back", x, y, width: 62, height: 7 })
    );
    midLayer.appendChild(col);
  }
  frag.appendChild(midLayer);

  // Layer 2.5: Subtle mineral/magma cleft seams nestled behind foreground columns
  const seamLayer = svgEl("g", { class: "strata-layer strata-layer--seams" });
  for (const seam of MAGMA_SEAMS) {
    seamLayer.append(
      svgEl("rect", {
        class: "strata-seam-halo",
        x: seam.x - 4,
        y: seam.y,
        width: 10,
        height: seam.h,
        fill: "url(#strata-seam-grad)",
      }),
      svgEl("rect", {
        class: "strata-seam",
        x: seam.x,
        y: seam.y + 8,
        width: 2,
        height: Math.max(0, seam.h - 8),
        fill: "url(#strata-seam-grad)",
      })
    );
  }
  frag.appendChild(seamLayer);

  // Layer 3: Near foreground basalt columns with crisp caps & edge bevels
  const nearLayer = svgEl("g", { class: "strata-layer strata-layer--near" });
  for (let i = 0; i < FRONT_COLUMNS.length; i += 1) {
    const h = FRONT_COLUMNS[i];
    const y = 320 - h;
    const x = i * 60 + 30;
    const col = svgEl("g", { class: "strata-col", style: `--i:${i + 5}` });
    col.append(
      svgEl("rect", { class: "strata-front", x, y, width: 58, height: h }),
      svgEl("rect", { class: "strata-bevel strata-bevel-front", x, y: y + 7, width: 3, height: Math.max(0, h - 7) }),
      svgEl("rect", { class: "strata-cap", x, y, width: 58, height: 7 })
    );
    nearLayer.appendChild(col);
  }
  frag.appendChild(nearLayer);

  // Horizon floor glow
  frag.appendChild(
    svgEl("rect", {
      class: "strata-glow",
      x: -24,
      y: 244,
      width: 1248,
      height: 76,
      fill: "url(#strata-magma)",
    })
  );
  svg.appendChild(frag);

  bindStrataParallax(svg);

  if (!isReducedMotionActive()) {
    svg.classList.add("is-entering");
    window.setTimeout(() => {
      svg.classList.remove("is-entering");
    }, 1400);
  }
}

let strataPulseTimer = 0;

/**
 * Triggers a subtle basalt column wave pulse when switching mod loader or MC version.
 */
export function pulseStrata(svg: SVGSVGElement): void {
  if (svg.classList.contains("is-entering") || isReducedMotionActive()) return;
  svg.classList.remove("is-pulsing");
  void svg.getBoundingClientRect();
  svg.classList.add("is-pulsing");
  if (strataPulseTimer) {
    window.clearTimeout(strataPulseTimer);
  }
  strataPulseTimer = window.setTimeout(() => {
    svg.classList.remove("is-pulsing");
    strataPulseTimer = 0;
  }, 560);
}
