import { animateDialogOpen, cancelElementAnimation, type EffectiveMotion } from "./motion";

export interface PaletteCommand {
  id: string;
  group: string;
  label: string;
  hint?: string;
  keywords?: string;
  icon: string;
  disabled?: boolean;
  run: () => void;
}

export interface PaletteDeps {
  getCommands(query: string): PaletteCommand[];
  locale(): string;
  isBlocked(): boolean;
  motion(): EffectiveMotion;
}

export interface CommandPaletteHandle {
  open(): void;
  close(): void;
}

const MAX_RESULTS = 40;

export function initCommandPalette(deps: PaletteDeps): CommandPaletteHandle {
  const dialog = document.getElementById("cmdk-dialog") as HTMLDialogElement | null;
  const input = document.getElementById("cmdk-input") as HTMLInputElement | null;
  const list = document.getElementById("cmdk-list") as HTMLUListElement | null;
  const emptyEl = document.getElementById("cmdk-empty") as HTMLElement | null;

  if (!dialog || !input || !list || !emptyEl) {
    return { open: () => {}, close: () => {} };
  }

  let openerEl: HTMLElement | null = null;
  let restoreFocusOnClose = true;
  let suppressReopenUntil = 0;
  let visibleCommands: PaletteCommand[] = [];
  let activeIndex = -1;

  const setActive = (nextIdx: number, scroll = true): void => {
    activeIndex = nextIdx;
    const optionEls = list.querySelectorAll<HTMLLIElement>(".cmdk-item");
    let activeOptId = "";
    optionEls.forEach((opt, i) => {
      const selected = i === activeIndex;
      opt.setAttribute("aria-selected", selected ? "true" : "false");
      if (selected) {
        activeOptId = opt.id;
        if (scroll) opt.scrollIntoView({ block: "nearest" });
      }
    });
    if (activeOptId) {
      input.setAttribute("aria-activedescendant", activeOptId);
    } else {
      input.removeAttribute("aria-activedescendant");
    }
  };

  const render = (rawQuery: string): void => {
    const loc = deps.locale();
    const q = rawQuery.trim().toLocaleLowerCase(loc);
    const all = deps.getCommands(rawQuery);

    let filtered: PaletteCommand[];
    if (!q) {
      filtered = all.slice(0, MAX_RESULTS);
    } else {
      const starts: PaletteCommand[] = [];
      const contains: PaletteCommand[] = [];
      for (const cmd of all) {
        const labelLower = cmd.label.toLocaleLowerCase(loc);
        const hay = `${cmd.label} ${cmd.keywords ?? ""} ${cmd.group}`.toLocaleLowerCase(loc);
        if (!hay.includes(q)) continue;
        if (labelLower.startsWith(q)) {
          starts.push(cmd);
        } else {
          contains.push(cmd);
        }
      }
      filtered = [...starts, ...contains].slice(0, MAX_RESULTS);
    }

    visibleCommands = filtered;
    list.innerHTML = "";
    emptyEl.hidden = filtered.length > 0;

    if (filtered.length === 0) {
      setActive(-1, false);
      return;
    }

    const frag = document.createDocumentFragment();
    let lastGroup = "";
    let firstEnabled = -1;

    filtered.forEach((cmd, idx) => {
      if (cmd.group !== lastGroup) {
        lastGroup = cmd.group;
        const header = document.createElement("li");
        header.className = "cmdk-group";
        header.setAttribute("role", "presentation");
        header.textContent = cmd.group;
        frag.appendChild(header);
      }

      if (firstEnabled === -1 && !cmd.disabled) {
        firstEnabled = idx;
      }

      const li = document.createElement("li");
      li.className = "cmdk-item";
      li.id = `cmdk-opt-${idx}`;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", "false");
      li.setAttribute("aria-disabled", cmd.disabled ? "true" : "false");

      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttribute("class", "ico small");
      svg.setAttribute("aria-hidden", "true");
      const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
      use.setAttribute("href", `#${cmd.icon}`);
      svg.appendChild(use);

      const labelSpan = document.createElement("span");
      labelSpan.className = "cmdk-label";
      labelSpan.textContent = cmd.label;

      const hintSpan = document.createElement("span");
      hintSpan.className = "cmdk-hint";
      hintSpan.textContent = cmd.hint ?? "";

      li.append(svg, labelSpan, hintSpan);

      li.addEventListener("mousemove", () => {
        if (activeIndex !== idx) {
          setActive(idx, false);
        }
      });

      li.addEventListener("click", () => {
        if (cmd.disabled) return;
        restoreFocusOnClose = false;
        suppressReopenUntil = performance.now() + 250;
        close();
        document.getElementById("main-content")?.focus({ preventScroll: true });
        cmd.run();
      });

      frag.appendChild(li);
    });

    list.appendChild(frag);
    setActive(firstEnabled, true);
  };

  const stepActive = (dir: 1 | -1): void => {
    const n = visibleCommands.length;
    if (n === 0) return;
    let idx = activeIndex;
    for (let step = 0; step < n; step += 1) {
      idx = idx < 0 ? (dir === 1 ? 0 : n - 1) : (idx + dir + n) % n;
      if (!visibleCommands[idx].disabled) {
        setActive(idx, true);
        return;
      }
    }
  };

  const open = (): void => {
    if (deps.isBlocked() || dialog.open || performance.now() < suppressReopenUntil) return;
    openerEl = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    openerEl?.blur();
    restoreFocusOnClose = true;
    dialog.showModal();
    input.value = "";
    render("");
    input.focus();
    animateDialogOpen(dialog, deps.motion());
  };

  const close = (): void => {
    cancelElementAnimation(dialog);
    if (dialog.open) dialog.close();
  };

  input.addEventListener("input", () => render(input.value));

  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      stepActive(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      stepActive(-1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      const cmd = activeIndex >= 0 ? visibleCommands[activeIndex] : undefined;
      if (cmd && !cmd.disabled) {
        restoreFocusOnClose = false;
        suppressReopenUntil = performance.now() + 300;
        close();
        document.getElementById("main-content")?.focus({ preventScroll: true });
        cmd.run();
      }
    }
  });

  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) {
      close();
    }
  });

  dialog.addEventListener("close", () => {
    cancelElementAnimation(dialog);
    const shouldRestore = restoreFocusOnClose;
    const target = openerEl;
    restoreFocusOnClose = true;
    openerEl = null;
    if (shouldRestore && target && target.isConnected) {
      requestAnimationFrame(() => {
        if (target.isConnected) target.focus();
      });
    }
  });

  return { open, close };
}
