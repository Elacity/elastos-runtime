import { publicWalletText, textNode } from "./wallet-render.js?v=wallet-20260720j";

export function createWalletFlows({
  modalNode,
  modalBackdropNode,
  heroNode = null,
  heroBackNode = null,
  accountsSectionNode = null,
  accountsBackNode = null,
  showStatus,
}) {
  // Rail 3D flip ~420ms; window morph is hard-sequenced (out, then in).
  const FLIP_MS = 420;
  const MORPH_FACE_MS = 200;
  let heroFlowActive = false;
  let accountsFlowActive = false;
  let heroResizeObserver = null;
  let accountsResizeObserver = null;
  let restoreFocusNode = null;
  let heroHeightReleaseTimer = 0;
  let accountsHeightReleaseTimer = 0;
  let heroMorphTimer = 0;
  let accountsMorphTimer = 0;

  function useWindowMorph() {
    return document.documentElement.dataset.walletPresentation !== "rail"
      && !prefersReducedMotion();
  }

  function openInfoModal(title, message) {
    openFlowModal(title, message, [], [modalButton("Done", closeModal)], { surface: "modal" });
  }

  function shouldUseAccountsFlip(title, options = {}) {
    if (options.surface === "modal" || options.surface === "hero") {
      return false;
    }
    if (options.surface === "accounts" || accountsFlowActive) {
      return Boolean(accountsSectionNode && accountsBackNode);
    }
    return Boolean(
      accountsSectionNode
        && accountsBackNode
        && /^(Create account|Import recovery key|Account|Rename account|Remove account|Delete account|Passkey required|Recovery key)\b/i.test(
          String(title || ""),
        ),
    );
  }

  function shouldUseHeroFlip(title, options = {}) {
    if (options.surface === "modal" || options.surface === "accounts") {
      return false;
    }
    if (options.surface === "hero" || heroFlowActive) {
      return Boolean(heroNode && heroBackNode);
    }
    // Send / Receive own the hero flip; other confirmations stay centered modals.
    return Boolean(heroNode && heroBackNode && /^(Send|Receive)\b/i.test(String(title || "")));
  }

  function openFlowModal(title, subtitle, nodes, actions = [modalButton("Cancel", closeModal, true)], options = {}) {
    const useAccounts = shouldUseAccountsFlip(title, options);
    const useHero = !useAccounts && shouldUseHeroFlip(title, options);
    const host = useAccounts ? accountsBackNode : useHero ? heroBackNode : modalNode;
    if (!host) {
      return;
    }
    host.replaceChildren();
    const header = document.createElement("header");
    // Receive (and short account labels) put the name beside the title to
    // reclaim the vertical stack under "Receive".
    header.className = options.headerInline
      ? "wallet-modal-header wallet-modal-header-inline"
      : "wallet-modal-header";
    const heading = textNode("h2", title);
    heading.tabIndex = -1;
    header.append(heading, textNode("p", subtitle, "wallet-state"));
    const body = document.createElement("div");
    body.className = "wallet-modal-body";
    for (const node of nodes) {
      body.append(node);
    }
    const footer = document.createElement("footer");
    footer.className = "wallet-modal-actions";
    for (const action of actions) {
      footer.append(action);
    }
    host.append(header, body, footer);
    if (useAccounts) {
      accountsFlowActive = true;
      accountsBackNode.classList.add("is-flow");
      accountsBackNode.setAttribute("role", "dialog");
      accountsBackNode.setAttribute("aria-modal", "true");
      accountsBackNode.setAttribute("aria-label", title);
      showAccountsBack(() => heading.focus({ preventScroll: true }));
      return;
    }
    if (useHero) {
      heroFlowActive = true;
      heroBackNode.classList.add("is-flow");
      heroBackNode.setAttribute("role", "dialog");
      heroBackNode.setAttribute("aria-modal", "true");
      heroBackNode.setAttribute("aria-label", title);
      showHeroBack(() => heading.focus({ preventScroll: true }));
      return;
    }
    modalNode.hidden = false;
    modalBackdropNode.hidden = false;
  }

  function showHeroBack(onReady = null) {
    if (!heroNode || !heroBackNode) {
      return;
    }
    captureRestoreFocus();
    window.clearTimeout(heroHeightReleaseTimer);
    window.clearTimeout(heroMorphTimer);
    // Lock to front height first, then ease to back after the flip starts.
    lockFlipShellHeight(
      heroNode.querySelector(".wallet-hero-flip"),
      heroNode.querySelector(".wallet-hero-face-front"),
    );
    setHeroFrontInert(true);
    const alreadyBack = heroNode.classList.contains("is-flipped");
    heroBackNode.hidden = false;
    // Reflow at opacity 0 before revealing — avoids a flash when leaving [hidden].
    void heroBackNode.offsetWidth;
    heroNode.dataset.face = "back";
    if (alreadyBack) {
      heroNode.classList.add("is-flipped", "is-morph-back");
      startHeroResizeObserver();
      queueHeroFlipMeasure();
      onReady?.();
      return;
    }
    if (useWindowMorph()) {
      heroNode.classList.remove("is-morph-back");
      heroNode.classList.add("is-flipped");
      heroMorphTimer = window.setTimeout(() => {
        heroMorphTimer = 0;
        heroNode.classList.add("is-morph-back");
        startHeroResizeObserver();
        queueHeroFlipMeasure();
        onReady?.();
      }, MORPH_FACE_MS);
      return;
    }
    heroNode.classList.add("is-flipped", "is-morph-back");
    startHeroResizeObserver();
    queueHeroFlipMeasure();
    onReady?.();
  }

  function hideHeroBack() {
    if (!heroNode || !heroBackNode) {
      return;
    }
    window.clearTimeout(heroMorphTimer);
    stopHeroResizeObserver();
    if (useWindowMorph() && heroNode.classList.contains("is-morph-back")) {
      heroNode.classList.remove("is-morph-back");
      heroNode.dataset.face = "front";
      heroMorphTimer = window.setTimeout(() => {
        heroMorphTimer = 0;
        heroNode.classList.remove("is-flipped");
        easeFlipShellToFront(
          heroNode.querySelector(".wallet-hero-flip"),
          heroNode.querySelector(".wallet-hero-face-front"),
          (timer) => {
            heroHeightReleaseTimer = timer;
          },
        );
        setHeroFrontInert(false);
        finishFlipUnmount(heroNode, heroBackNode, MORPH_FACE_MS);
      }, MORPH_FACE_MS);
      return;
    }
    heroNode.dataset.face = "front";
    heroNode.classList.remove("is-flipped", "is-morph-back");
    easeFlipShellToFront(
      heroNode.querySelector(".wallet-hero-flip"),
      heroNode.querySelector(".wallet-hero-face-front"),
      (timer) => {
        heroHeightReleaseTimer = timer;
      },
    );
    setHeroFrontInert(false);
    finishFlipUnmount(heroNode, heroBackNode);
  }

  function showAccountsBack(onReady = null) {
    if (!accountsSectionNode || !accountsBackNode) {
      return;
    }
    captureRestoreFocus();
    window.clearTimeout(accountsHeightReleaseTimer);
    window.clearTimeout(accountsMorphTimer);
    lockFlipShellHeight(
      accountsSectionNode.querySelector(".wallet-accounts-flip"),
      accountsSectionNode.querySelector(".wallet-accounts-face-front"),
    );
    setAccountsFrontInert(true);
    const alreadyBack = accountsSectionNode.classList.contains("is-flipped");
    accountsBackNode.hidden = false;
    void accountsBackNode.offsetWidth;
    accountsSectionNode.dataset.face = "back";
    accountsSectionNode.scrollIntoView({ behavior: "smooth", block: "nearest" });
    if (alreadyBack) {
      // Create → chain step: stay on the back face, no re-overlap with cards.
      accountsSectionNode.classList.add("is-flipped", "is-morph-back");
      startAccountsResizeObserver();
      queueAccountsFlipMeasure();
      onReady?.();
      return;
    }
    if (useWindowMorph()) {
      // Phase 1: account cards fade out; back stays at opacity 0.
      accountsSectionNode.classList.remove("is-morph-back");
      accountsSectionNode.classList.add("is-flipped");
      accountsMorphTimer = window.setTimeout(() => {
        accountsMorphTimer = 0;
        // Phase 2: only now fade Create/Import (or menu) in.
        accountsSectionNode.classList.add("is-morph-back");
        startAccountsResizeObserver();
        queueAccountsFlipMeasure();
        onReady?.();
      }, MORPH_FACE_MS);
      return;
    }
    accountsSectionNode.classList.add("is-flipped", "is-morph-back");
    startAccountsResizeObserver();
    queueAccountsFlipMeasure();
    onReady?.();
  }

  function hideAccountsBack() {
    if (!accountsSectionNode || !accountsBackNode) {
      return;
    }
    window.clearTimeout(accountsMorphTimer);
    stopAccountsResizeObserver();
    if (useWindowMorph() && accountsSectionNode.classList.contains("is-morph-back")) {
      accountsSectionNode.classList.remove("is-morph-back");
      accountsSectionNode.dataset.face = "front";
      accountsMorphTimer = window.setTimeout(() => {
        accountsMorphTimer = 0;
        accountsSectionNode.classList.remove("is-flipped");
        easeFlipShellToFront(
          accountsSectionNode.querySelector(".wallet-accounts-flip"),
          accountsSectionNode.querySelector(".wallet-accounts-face-front"),
          (timer) => {
            accountsHeightReleaseTimer = timer;
          },
        );
        setAccountsFrontInert(false);
        finishFlipUnmount(accountsSectionNode, accountsBackNode, MORPH_FACE_MS);
      }, MORPH_FACE_MS);
      return;
    }
    accountsSectionNode.dataset.face = "front";
    accountsSectionNode.classList.remove("is-flipped", "is-morph-back");
    easeFlipShellToFront(
      accountsSectionNode.querySelector(".wallet-accounts-flip"),
      accountsSectionNode.querySelector(".wallet-accounts-face-front"),
      (timer) => {
        accountsHeightReleaseTimer = timer;
      },
    );
    setAccountsFrontInert(false);
    finishFlipUnmount(accountsSectionNode, accountsBackNode);
  }

  function lockFlipShellHeight(flip, front) {
    if (!flip || !front) {
      return;
    }
    const height = Math.ceil(front.getBoundingClientRect().height);
    flip.style.minHeight = `${Math.max(height, 1)}px`;
  }

  function easeFlipShellToFront(flip, front, setTimer) {
    if (!flip) {
      return;
    }
    if (prefersReducedMotion()) {
      flip.style.minHeight = "";
      return;
    }
    const frontHeight = front
      ? Math.ceil(front.getBoundingClientRect().height)
      : Math.ceil(flip.getBoundingClientRect().height);
    flip.style.minHeight = `${Math.max(frontHeight, 1)}px`;
    const timer = window.setTimeout(() => {
      if (flip.style.minHeight === `${Math.max(frontHeight, 1)}px`) {
        flip.style.minHeight = "";
      }
    }, FLIP_MS);
    setTimer?.(timer);
  }

  function captureRestoreFocus() {
    if (!restoreFocusNode) {
      restoreFocusNode = document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    }
  }

  function finishFlipUnmount(sectionNode, backNode, delayMs = FLIP_MS) {
    const focusTarget = restoreFocusNode;
    restoreFocusNode = null;
    const finish = () => {
      if (sectionNode.dataset.face === "front") {
        backNode.hidden = true;
        backNode.classList.remove("is-flow");
        backNode.removeAttribute("role");
        backNode.removeAttribute("aria-modal");
        backNode.removeAttribute("aria-label");
        backNode.replaceChildren();
        sectionNode.classList.remove("is-morph-back");
      }
      if (focusTarget?.isConnected) {
        focusTarget.focus({ preventScroll: true });
      }
    };
    window.setTimeout(finish, prefersReducedMotion() ? 0 : delayMs);
  }

  function setHeroFrontInert(inert) {
    const front = heroNode?.querySelector(".wallet-hero-face-front");
    if (!front) {
      return;
    }
    front.inert = inert;
    front.setAttribute("aria-hidden", inert ? "true" : "false");
  }

  function setAccountsFrontInert(inert) {
    const front = accountsSectionNode?.querySelector(".wallet-accounts-face-front");
    const actions = accountsSectionNode?.querySelector(".wallet-section-actions");
    if (front) {
      front.inert = inert;
      front.setAttribute("aria-hidden", inert ? "true" : "false");
    }
    if (actions) {
      actions.inert = inert;
      actions.setAttribute("aria-hidden", inert ? "true" : "false");
    }
  }

  function startHeroResizeObserver() {
    stopHeroResizeObserver();
    if (!heroBackNode || typeof ResizeObserver !== "function") {
      return;
    }
    heroResizeObserver = new ResizeObserver(() => {
      syncHeroFlipHeight();
    });
    heroResizeObserver.observe(heroBackNode);
  }

  function stopHeroResizeObserver() {
    heroResizeObserver?.disconnect();
    heroResizeObserver = null;
  }

  function startAccountsResizeObserver() {
    stopAccountsResizeObserver();
    if (!accountsBackNode || typeof ResizeObserver !== "function") {
      return;
    }
    accountsResizeObserver = new ResizeObserver(() => {
      syncAccountsFlipHeight();
    });
    accountsResizeObserver.observe(accountsBackNode);
  }

  function stopAccountsResizeObserver() {
    accountsResizeObserver?.disconnect();
    accountsResizeObserver = null;
  }

  function queueHeroFlipMeasure() {
    requestAnimationFrame(() => {
      syncHeroFlipHeight();
      requestAnimationFrame(syncHeroFlipHeight);
    });
  }

  function queueAccountsFlipMeasure() {
    requestAnimationFrame(() => {
      syncAccountsFlipHeight();
      requestAnimationFrame(syncAccountsFlipHeight);
    });
  }

  function syncHeroFlipHeight() {
    if (!heroNode || !heroBackNode || !heroNode.classList.contains("is-flipped")) {
      return;
    }
    const flip = heroNode.querySelector(".wallet-hero-flip");
    const front = heroNode.querySelector(".wallet-hero-face-front");
    if (!flip || !front) {
      return;
    }
    const frontHeight = Math.ceil(front.getBoundingClientRect().height);
    const backHeight = Math.ceil(heroBackNode.scrollHeight);
    const target = Math.max(frontHeight, backHeight);
    // CSS transitions min-height — mid-flow step changes ease too.
    flip.style.minHeight = `${target}px`;
  }

  function syncAccountsFlipHeight() {
    if (!accountsSectionNode || !accountsBackNode || !accountsSectionNode.classList.contains("is-flipped")) {
      return;
    }
    const flip = accountsSectionNode.querySelector(".wallet-accounts-flip");
    const front = accountsSectionNode.querySelector(".wallet-accounts-face-front");
    if (!flip || !front) {
      return;
    }
    const frontHeight = Math.ceil(front.getBoundingClientRect().height);
    const backHeight = Math.ceil(accountsBackNode.scrollHeight);
    flip.style.minHeight = `${Math.max(frontHeight, backHeight, 120)}px`;
  }

  function prefersReducedMotion() {
    return Boolean(window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches);
  }

  function closeModal() {
    if (accountsFlowActive) {
      accountsFlowActive = false;
      hideAccountsBack();
      return;
    }
    if (heroFlowActive) {
      heroFlowActive = false;
      hideHeroBack();
      return;
    }
    modalNode.hidden = true;
    modalBackdropNode.hidden = true;
  }

  function flowHost() {
    if (accountsFlowActive && accountsBackNode) {
      return accountsBackNode;
    }
    if (heroFlowActive && heroBackNode) {
      return heroBackNode;
    }
    return modalNode;
  }

  function flowRow(title, subtitle, onClick) {
    const row = document.createElement("button");
    row.className = "wallet-flow-row";
    row.type = "button";
    row.append(textNode("div", "", ""));
    row.firstChild.append(textNode("strong", title), textNode("span", subtitle));
    row.append(textNode("span", "›", "wallet-state"));
    row.addEventListener("click", () => {
      Promise.resolve(onClick(row)).catch((error) => {
        const message = error && error.message ? error.message : error;
        showStatus(publicWalletText(message), "error");
      });
    });
    return row;
  }

  function flowStaticRow(title, value) {
    const row = document.createElement("div");
    row.className = "wallet-flow-row";
    row.append(textNode("strong", title), textNode("span", value));
    return row;
  }

  function modalButton(label, onClick, secondary = false, danger = false) {
    const button = document.createElement("button");
    button.className = [
      "wallet-button",
      secondary ? "wallet-button-secondary" : "",
      danger ? "wallet-button-danger" : "",
    ].filter(Boolean).join(" ");
    button.type = "button";
    button.textContent = label;
    button.addEventListener("click", () => {
      Promise.resolve(onClick(button)).catch((error) => {
        const message = error && error.message ? error.message : error;
        showStatus(publicWalletText(message), "error");
      });
    });
    return button;
  }

  return {
    closeModal,
    flowHost,
    flowRow,
    flowStaticRow,
    modalButton,
    openFlowModal,
    openInfoModal,
  };
}
