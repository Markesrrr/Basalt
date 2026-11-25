/**
 * Basalt Kinetic WebUI Motion Engine
 * Adapted from KINETIC_WEBUI_MOTION_SKILL/src/motion-engine.ts.
 * Framework-neutral, zero runtime dependencies, zero idle frame loops.
 * All animations are interruptible via WeakMap cancellation and respect
 * both OS prefers-reduced-motion and Basalt's in-app ui_reduced_motion setting.
 */

export type MotionChoice = "system" | "full" | "subtle" | "none";
export type EffectiveMotion = "full" | "subtle" | "none";
export type MotionResult = "finished" | "canceled" | "skipped";

const EASE_ENTER = "cubic-bezier(0.16, 1, 0.3, 1)";
const EASE_CONTROL = "cubic-bezier(0.2, 0.8, 0.2, 1)";

const activeAnimations = new WeakMap<HTMLElement, Animation>();

export function systemPrefersReduced(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

export function resolveEffectiveMotion(userReducedMotion: boolean): EffectiveMotion {
  if (userReducedMotion || systemPrefersReduced()) {
    return "none";
  }
  return "full";
}

export function syncRootMotionAttribute(userReducedMotion: boolean): EffectiveMotion {
  const effective = resolveEffectiveMotion(userReducedMotion);
  document.documentElement.setAttribute("data-motion", effective);
  document.body.classList.toggle("reduce-motion", effective === "none");
  return effective;
}

export function watchSystemMotion(cb: (reduced: boolean) => void): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return () => {};
  }
  const media = window.matchMedia("(prefers-reduced-motion: reduce)");
  const listener = (ev: MediaQueryListEvent) => cb(ev.matches);
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
}

/**
 * Runs an interruptible Web Animations API transition on `el`.
 * Cancels any in-flight animation on the same element immediately.
 * Never use completion callbacks for business state.
 */
export function animateElement(
  el: HTMLElement,
  keyframes: Keyframe[] | PropertyIndexedKeyframes,
  timing: KeyframeAnimationOptions,
  preference: EffectiveMotion,
  onSkip?: () => void
): Promise<MotionResult> {
  activeAnimations.get(el)?.cancel();
  if (
    preference === "none" ||
    document.hidden ||
    !el.isConnected ||
    typeof el.animate !== "function"
  ) {
    onSkip?.();
    return Promise.resolve("skipped");
  }
  const normalDuration = typeof timing.duration === "number" ? timing.duration : 180;
  const duration =
    preference === "subtle"
      ? Math.max(60, Math.min(150, Math.round(normalDuration * 0.62)))
      : normalDuration;
  const anim = el.animate(keyframes, { ...timing, duration });
  activeAnimations.set(el, anim);
  return new Promise<MotionResult>((resolve) => {
    let settled = false;
    const finish = (outcome: MotionResult) => {
      if (settled) return;
      settled = true;
      if (activeAnimations.get(el) === anim) {
        activeAnimations.delete(el);
      }
      resolve(outcome);
    };
    anim.addEventListener("finish", () => finish("finished"), { once: true });
    anim.addEventListener("cancel", () => finish("canceled"), { once: true });
  });
}

export function cancelElementAnimation(el: HTMLElement): void {
  activeAnimations.get(el)?.cancel();
  activeAnimations.delete(el);
}

/**
 * Spatial view handoff (Play / Saves / Profile / Settings).
 * Visibility and ARIA state update synchronously on frame 0;
 * WAAPI provides a subtle, interruptible 185ms entrance when motion is enabled.
 */
export function animateViewHandoff(
  incomingView: HTMLElement,
  preference: EffectiveMotion
): void {
  if (preference === "none") {
    cancelElementAnimation(incomingView);
    return;
  }
  void animateElement(
    incomingView,
    [
      { opacity: 0, transform: "translateY(6px)" },
      { opacity: 1, transform: "translateY(0)" },
    ],
    {
      duration: 185,
      easing: EASE_ENTER,
    },
    preference
  );
}

/**
 * Grounded modal dialog entrance (`<dialog>`).
 * Cancels immediately if Escape or close is triggered mid-flight.
 */
export function animateDialogOpen(
  dialogEl: HTMLDialogElement,
  preference: EffectiveMotion
): void {
  if (preference === "none") {
    cancelElementAnimation(dialogEl);
    return;
  }
  void animateElement(
    dialogEl,
    [
      { opacity: 0, transform: "translateY(8px) scale(0.992)" },
      { opacity: 1, transform: "translateY(0) scale(1)" },
    ],
    {
      duration: 195,
      easing: EASE_ENTER,
    },
    preference
  );
}

/**
 * Truthful toast entrance. No spring overshoot on errors.
 */
export function animateToastIn(
  toastEl: HTMLElement,
  preference: EffectiveMotion,
  tone: "neutral" | "success" | "error"
): void {
  if (preference === "none") {
    cancelElementAnimation(toastEl);
    return;
  }
  const duration = tone === "error" ? 150 : 185;
  const easing = tone === "error" ? EASE_CONTROL : EASE_ENTER;
  void animateElement(
    toastEl,
    [
      { opacity: 0, transform: "translateY(6px)" },
      { opacity: 1, transform: "translateY(0)" },
    ],
    {
      duration,
      easing,
    },
    preference
  );
}

/**
 * Accordion / `<details>` reveal without `height: auto` layout stutter.
 * Supports Escape to close when focus is inside the drawer body.
 */
export function wireDetailsAccordion(
  details: HTMLDetailsElement,
  getPreference: () => EffectiveMotion
): void {
  const body = details.querySelector<HTMLElement>(".drawer-body");
  if (!body) return;

  details.addEventListener("toggle", () => {
    const pref = getPreference();
    if (!details.open || pref === "none") {
      cancelElementAnimation(body);
      return;
    }
    void animateElement(
      body,
      [
        { opacity: 0, transform: "translateY(-4px)" },
        { opacity: 1, transform: "translateY(0)" },
      ],
      {
        duration: 175,
        easing: EASE_ENTER,
      },
      pref
    );
  });

  details.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && details.open) {
      const summary = details.querySelector<HTMLElement>("summary");
      details.open = false;
      summary?.focus();
      e.stopPropagation();
    }
  });
}

/**
 * WAI-ARIA roving arrow-key navigation for segmented controls and navigation rails.
 * Supports ArrowLeft / ArrowRight / ArrowUp / ArrowDown / Home / End.
 */
export function wireRovingTabGroup(
  container: HTMLElement,
  buttonSelector: string
): void {
  container.addEventListener("keydown", (e) => {
    if (
      ![
        "ArrowLeft",
        "ArrowRight",
        "ArrowUp",
        "ArrowDown",
        "Home",
        "End",
      ].includes(e.key)
    ) {
      return;
    }
    const items = Array.from(
      container.querySelectorAll<HTMLButtonElement>(buttonSelector)
    ).filter((btn) => !btn.disabled && !btn.hidden);
    if (items.length <= 1) return;

    const currentIdx = items.indexOf(document.activeElement as HTMLButtonElement);
    if (currentIdx === -1) return;

    e.preventDefault();
    let nextIdx = currentIdx;
    if (e.key === "Home") {
      nextIdx = 0;
    } else if (e.key === "End") {
      nextIdx = items.length - 1;
    } else if (e.key === "ArrowRight" || e.key === "ArrowDown") {
      nextIdx = (currentIdx + 1) % items.length;
    } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
      nextIdx = (currentIdx - 1 + items.length) % items.length;
    }

    const target = items[nextIdx];
    if (target && target !== items[currentIdx]) {
      target.focus();
      target.click();
    }
  });
}

const slidingGroupSyncers: Array<() => void> = [];

/**
 * Re-align every registered sliding-pill indicator (after view switches, resize,
 * i18n text updates, or preference hydration).
 */
export function syncSlidingIndicators(): void {
  for (const sync of slidingGroupSyncers) {
    sync();
  }
}

/**
 * Attaches an animated, sticky sliding-pill background indicator to a segmented control
 * or tab bar without shifting layout. Snaps cleanly without an initial fly-in from (0, 0)
 * when a hidden container becomes visible, and glides smoothly on interactive changes.
 */
export function wireSlidingGroup(
  container: HTMLElement,
  activeSelector = ".active, [aria-selected='true'], [aria-pressed='true']"
): () => void {
  container.classList.add("has-glider");
  let glider = container.querySelector<HTMLSpanElement>(":scope > .seg-glider");
  if (!glider) {
    glider = document.createElement("span");
    glider.className = "seg-glider";
    glider.setAttribute("aria-hidden", "true");
    container.prepend(glider);
  }

  let rafId = 0;

  const sync = (): void => {
    const active = container.querySelector<HTMLElement>(activeSelector);
    if (!active || active.offsetWidth === 0 || active.offsetHeight === 0 || container.offsetParent === null) {
      if (rafId) {
        cancelAnimationFrame(rafId);
        rafId = 0;
      }
      glider.classList.remove("is-ready", "is-positioned");
      delete glider.dataset.positioned;
      return;
    }

    const x = active.offsetLeft;
    const y = active.offsetTop;
    const w = active.offsetWidth;
    const h = active.offsetHeight;
    const firstPlacement = glider.dataset.positioned !== "1";

    glider.style.setProperty("--gx", `${x}px`);
    glider.style.setProperty("--gy", `${y}px`);
    glider.style.setProperty("--gw", `${w}px`);
    glider.style.setProperty("--gh", `${h}px`);

    if (firstPlacement) {
      glider.classList.remove("is-ready");
      glider.classList.add("is-positioned");
      void glider.offsetWidth;
      glider.dataset.positioned = "1";
      if (rafId) cancelAnimationFrame(rafId);
      rafId = requestAnimationFrame(() => {
        rafId = 0;
        if (glider.dataset.positioned === "1") {
          glider.classList.add("is-ready");
        }
      });
    } else {
      glider.classList.add("is-positioned", "is-ready");
    }
  };

  slidingGroupSyncers.push(sync);
  if (typeof ResizeObserver !== "undefined") {
    const ro = new ResizeObserver(() => sync());
    ro.observe(container);
    for (const child of Array.from(container.children)) {
      if (child !== glider && child instanceof HTMLElement) {
        ro.observe(child);
      }
    }
  }
  sync();
  return sync;
}

/**
 * Crisp compositor text swap used when the selected hero target/title changes.
 */
export function animateTextSwap(
  el: HTMLElement,
  preference: EffectiveMotion
): void {
  if (preference === "none") {
    cancelElementAnimation(el);
    return;
  }
  void animateElement(
    el,
    [
      { opacity: 0.35, transform: "translateY(6px)" },
      { opacity: 1, transform: "translateY(0)" },
    ],
    {
      duration: 195,
      easing: EASE_ENTER,
    },
    preference
  );
}
