const SVG_NS = "http://www.w3.org/2000/svg";

const BACK_COLUMNS = [
  150, 190, 120, 210, 170, 240, 140, 200, 260, 180,
  220, 130, 190, 160, 230, 170, 140, 200, 250, 150,
];

const FRONT_COLUMNS = [
  90, 130, 70, 150, 110, 80, 160, 100, 60, 140,
  120, 90, 170, 110, 80, 130, 100, 150, 70, 120,
];

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

export function seedHue(seed: string): number {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i += 1) {
    h ^= seed.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return (h >>> 0) % 360;
}

export function renderStrata(svg: SVGSVGElement): void {
  if (svg.childElementCount > 0) return;

  const defs = svgEl("defs", {});
  const grad = svgEl("linearGradient", { id: "strata-magma", x1: 0, y1: 0, x2: 0, y2: 1 });
  grad.append(
    svgEl("stop", { offset: "0%", style: "stop-color: var(--accent); stop-opacity: 0" }),
    svgEl("stop", { offset: "100%", style: "stop-color: var(--accent); stop-opacity: .55" })
  );
  defs.appendChild(grad);
  svg.appendChild(defs);

  const frag = document.createDocumentFragment();
  for (let i = 0; i < BACK_COLUMNS.length; i += 1) {
    const h = BACK_COLUMNS[i];
    const y = 320 - h;
    const x = i * 60;
    const col = svgEl("g", { class: "strata-col", style: `--i:${i}` });
    col.append(
      svgEl("rect", { class: "strata-back", x, y, width: 62, height: h }),
      svgEl("rect", { class: "strata-cap strata-cap-back", x, y, width: 62, height: 7 })
    );
    frag.appendChild(col);
  }

  for (let i = 0; i < FRONT_COLUMNS.length; i += 1) {
    const h = FRONT_COLUMNS[i];
    const y = 320 - h;
    const x = i * 60 + 30;
    const col = svgEl("g", { class: "strata-col", style: `--i:${i + 4}` });
    col.append(
      svgEl("rect", { class: "strata-front", x, y, width: 58, height: h }),
      svgEl("rect", { class: "strata-cap", x, y, width: 58, height: 7 })
    );
    frag.appendChild(col);
  }

  frag.appendChild(
    svgEl("rect", { class: "strata-glow", x: 0, y: 250, width: 1200, height: 70, fill: "url(#strata-magma)" })
  );
  svg.appendChild(frag);

  svg.classList.add("is-entering");
  window.setTimeout(() => {
    svg.classList.remove("is-entering");
  }, 1400);
}

let strataPulseTimer = 0;

/**
 * Triggers a subtle basalt column wave pulse when switching mod loader or MC version.
 */
export function pulseStrata(svg: SVGSVGElement): void {
  if (svg.classList.contains("is-entering")) return;
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
