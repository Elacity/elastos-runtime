// The exact command the owner runs on the Home computer to arm recovery for
// this page. The passkey rp id is the page's host name. Both values are
// single-quoted so a pasted command never runs anything else.
const shellQuote = (value) => `'${String(value).replaceAll("'", "'\\''")}'`;

export function ownerRecoveryCommand({ origin, hostname }) {
  return `elastos identity arm-owner-recovery --origin ${shellQuote(origin)} --rp-id ${shellQuote(hostname)}`;
}
