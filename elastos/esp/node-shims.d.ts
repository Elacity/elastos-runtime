/**
 * Minimal ambient declarations for the Node built-in test modules used by the
 * ESP conformance tests, so the package type-checks WITHOUT pulling @types/node
 * (this repo has no npm install step). Node provides the real implementations at
 * runtime; these only describe the tiny surface the tests touch.
 */
declare module "node:test" {
  export function describe(name: string, fn: () => void): void;
  export function it(name: string, fn: () => void | Promise<void>): void;
}

declare module "node:assert/strict" {
  interface Assert {
    (value: unknown, message?: string): void;
    ok(value: unknown, message?: string): void;
    equal(actual: unknown, expected: unknown, message?: string): void;
  }
  const assert: Assert;
  export default assert;
}
