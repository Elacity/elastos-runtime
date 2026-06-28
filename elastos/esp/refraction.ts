/**
 * ESP v0 — the refraction toggle (W6), as a pure state model.
 *
 * Refraction is the focus-swap between two registered shells over IDENTICAL
 * projected state — the same runtime facts, viewed through a different lens. The
 * load-bearing property: there is ONE source of authority and NO state migration
 * across the swap; only which shell has focus changes. (The visual ~320ms
 * cross-fade is W5b/browser; this is the verifiable state model under it.)
 */

/** Two registered shells over one projected state. `focused` is always one of
 *  `shells`; `projected` is carried UNCHANGED across a toggle. */
export interface RefractionState<T> {
  readonly shells: readonly [string, string];
  readonly focused: string;
  readonly projected: T;
}

/** Register a refraction between two shells, focused on the first. */
export function makeRefraction<T>(a: string, b: string, projected: T): RefractionState<T> {
  return { shells: [a, b], focused: a, projected };
}

/**
 * Swap focus to the other shell. The projected state is carried through
 * unchanged — a refraction is a change of lens, not a migration of state.
 */
export function toggleFocus<T>(state: RefractionState<T>): RefractionState<T> {
  const [a, b] = state.shells;
  const next = state.focused === a ? b : a;
  return { ...state, focused: next };
}
